"""Identity, import, retrieval and source lifecycle commands."""

import json
import mimetypes
from pathlib import Path
from typing import Annotated, Literal
from uuid import uuid4

import typer
from nous_sdk import NousClient
from nous_sdk.contracts import (
    AdmitFiles,
    AdoptCognition,
    AdoptMedia,
    BundleRequest,
    FreezeCatalog,
    ImportDocument,
    InstallBundle,
    MediaReference,
    MergeConcepts,
    ProcessCatalog,
    QueryRequest,
    ResegmentEpisodes,
    SourceCommand,
    SourceRegion,
    SplitConcept,
)
from nous_sdk.contracts.dataset_imports import BackfillSourceTimes, ImportDataset
from nous_sdk.contracts.lifecycle import MemoryChange
from nous_sdk.contracts.records import ReadRecords, RecordsQuery
from nous_sdk.contracts.vector_projection import ProjectVectors
from nous_sdk.operations import encoded, segment
from pydantic import JsonValue, TypeAdapter

from .common import invoke, read_command, read_json


def register(cli: typer.Typer) -> None:
    from .resources import register as register_resources

    register_resources(cli)
    from .corrections import register as register_corrections

    register_corrections(cli)
    from .episodes import register as register_episodes

    register_episodes(cli)
    from .facts import register as register_facts

    register_facts(cli)

    dataset = typer.Typer(help="Import captured datasets and recover explicit source times.")
    cli.add_typer(dataset, name="dataset")

    @dataset.command("import")
    def import_dataset(ctx: typer.Context, namespace_id: str, request: Path) -> None:
        """Submit an ImportDataset request referring to uploaded source originals."""
        command = read_command(request, ImportDataset)
        invoke(ctx, lambda api: api.import_dataset(namespace_id, command))

    @dataset.command("source-times")
    def backfill_source_times(ctx: typer.Context, namespace_id: str, request: Path) -> None:
        """Recover missing times from actual stored source fields."""
        command = read_command(request, BackfillSourceTimes)
        invoke(ctx, lambda api: api.backfill_source_times(namespace_id, command))

    @cli.command("project-vectors")
    def project_vectors(ctx: typer.Context, namespace: str, subject: str, request: Path) -> None:
        """Submit exact cognitive/media targets from a ProjectVectors JSON file."""
        command = read_command(request, ProjectVectors)
        invoke(ctx, lambda api: api.project_vectors(namespace, command))

    namespace = typer.Typer(help="Namespace directory and creation.", no_args_is_help=False)
    from .identities import register as register_identities

    register_identities(namespace)
    source = typer.Typer(help="Original sources and their lifecycle.", no_args_is_help=False)
    catalog = typer.Typer(help="Freeze and process input catalogs.", no_args_is_help=False)
    cognition = typer.Typer(help="Exact revisions and cognitive evolution.", no_args_is_help=False)
    bundle = typer.Typer(help="Logical subject export and install.", no_args_is_help=False)
    media = typer.Typer(help="Upload and adopt original media.", no_args_is_help=False)
    from .media import register as register_media

    register_media(media)
    for name, group in (
        ("namespace", namespace),
        ("source", source),
        ("catalog", catalog),
        ("cognition", cognition),
        ("bundle", bundle),
        ("media", media),
    ):
        cli.add_typer(group, name=name)

    @namespace.command("list")
    def namespaces(ctx: typer.Context) -> None:
        invoke(ctx, lambda api: api.namespaces.list())

    @namespace.command("create")
    def create_namespace(ctx: typer.Context, name: str, operation_id: str | None = None) -> None:
        invoke(ctx, lambda api: api.create_namespace(name, operation_id=operation_id))

    @namespace.command("diagnostics")
    def diagnostics(ctx: typer.Context, namespace_id: str) -> None:
        invoke(ctx, lambda api: api.namespace_diagnostics(namespace_id))

    @cli.command("import")
    def import_documents(
        ctx: typer.Context,
        namespace_id: str,
        paths: list[Path],
        llm: bool = True,
        vectors: Literal["disabled", "metadata_allowed", "required"] = "metadata_allowed",
        operation_id: str | None = None,
    ) -> None:
        """Upload documents and media in their original formats, then import their content."""
        from nous_sdk.imports import upload_import_file_documents

        operation = operation_id or str(uuid4())

        async def run(api: NousClient):
            documents: list[ImportDocument] = []
            for index, selected in enumerate(paths):
                path = selected.resolve(strict=True)
                documents.extend(
                    await upload_import_file_documents(
                        api,
                        namespace_id,
                        ({"path": str(path), "file_identity": str(path)},),
                        transfer_root=path.parent,
                        operation_id=f"{operation}:upload:{index}",
                    )
                )
            return await api.admit_documents(
                namespace_id,
                AdmitFiles(
                    operation_id=operation,
                    llm_enabled=llm,
                    vectors=vectors,
                    inputs=tuple(documents),
                ),
            )

        invoke(ctx, run)

    @cli.command("query")
    def query(
        ctx: typer.Context,
        namespace_id: str,
        text: str = typer.Argument(""),
        mode: Literal["lexical", "dense", "hybrid", "vcp"] = "lexical",
        limit: int = 10,
        request: Path | None = None,
        materialize: bool = False,
        rerank: Literal["off", "optional", "required"] = "off",
        rerank_invocation_id: str | None = None,
    ) -> None:
        """Query cognition; --request accepts the full public QueryRequest JSON."""
        payload = (
            read_command(request, QueryRequest)
            if request
            else QueryRequest(
                text=text,
                mode=mode,
                limit=limit,
                materialize=materialize,
                rerank=rerank,
                rerank_invocation_id=rerank_invocation_id,
            )
        )
        invoke(ctx, lambda api: api.query_cognition(namespace_id, payload))

    @cli.command("manual")
    def manual(ctx: typer.Context) -> None:
        invoke(ctx, lambda api: api.query_manual())

    @cli.command("profile")
    def profile(
        ctx: typer.Context,
        namespace_id: str,
        action: Literal["list", "query", "evidence", "get_aliases"] = "query",
        person_id: str = "",
        person_keyword: str = "",
        limit: int = 10,
        force_refresh: bool = False,
        request: Path | None = None,
    ) -> None:
        """Browse retained person profiles and exact evidence."""

        async def read(api: NousClient) -> object:
            from nous_sdk.contracts.person_profiles import PersonProfilesQuery, RefreshPersonProfile

            command = (
                read_command(request, PersonProfilesQuery)
                if request
                else PersonProfilesQuery(
                    identifier=person_id,
                    name=person_keyword,
                    limit=limit,
                    evidence_limit=limit,
                )
            )
            result = await api.person_profiles(namespace_id, command)
            if force_refresh:
                if len(result.items) != 1 or not (
                    command.reference or command.identifier or command.name
                ):
                    raise ValueError("Profile refresh requires one explicit Person")
                receipt = await api.refresh_person_profile(
                    namespace_id,
                    RefreshPersonProfile(reference=result.items[0].record.reference),
                )
                await api.wait_success([receipt.job_id], namespace_id=namespace_id)
                result = await api.person_profiles(
                    namespace_id,
                    command.model_copy(
                        update={
                            "reference": None,
                            "identifier": result.items[0].record.reference.id,
                        }
                    ),
                )
            if action == "get_aliases":
                from nous_sdk.contracts import EntityBody

                return [
                    {
                        "reference": item.record.reference,
                        "name": item.record.body.label,
                        "aliases": item.record.body.effective_aliases,
                    }
                    for item in result.items
                    if isinstance(item.record.body, EntityBody)
                ]
            return result

        invoke(ctx, read)

    @cli.command("graph-search")
    def graph_search(ctx: typer.Context, namespace_id: str, query: str, limit: int = 50) -> None:
        """Search authorized canonical graph entities and relations."""
        from nous_sdk.contracts.graph_management import GraphSearchQuery

        invoke(
            ctx,
            lambda api: api.search_graph(namespace_id, GraphSearchQuery(text=query, limit=limit)),
        )

    @cli.command("graph")
    def graph(
        ctx: typer.Context,
        namespace_id: str,
        action: Annotated[str, typer.Argument()] = "get_graph",
        request: Path | None = None,
    ) -> None:
        """Read graph details or apply a source-backed manual graph action from JSON."""
        from nous_sdk.contracts.graph_management import GraphChange, GraphReadQuery

        if action == "get_graph":
            invoke(ctx, lambda api: api.graph_snapshot(namespace_id))
        elif action in {"node_detail", "edge_detail"}:
            if request is None:
                raise ValueError("Graph details require a GraphReadQuery JSON request")
            selection = read_command(request, GraphReadQuery)
            invoke(ctx, lambda api: api.read_graph(namespace_id, selection))
        else:
            if request is None:
                raise ValueError("Graph changes require a GraphChange JSON request")
            values = TypeAdapter(dict[str, JsonValue]).validate_python(read_json(request))
            command = GraphChange.model_validate({**values, "action": action})
            invoke(ctx, lambda api: api.manage_graph(namespace_id, command))

    @source.command("list")
    def sources(ctx: typer.Context, namespace_id: str) -> None:
        invoke(ctx, lambda api: api.source_directory(namespace_id))

    @source.command("read")
    def read_source(ctx: typer.Context, namespace_id: str, reference: str) -> None:
        invoke(ctx, lambda api: api.source(namespace_id, reference))

    @source.command("preview")
    def preview(
        ctx: typer.Context,
        namespace_id: str,
        observations: list[str] | None = None,
        request: Path | None = None,
        action: Literal["trash", "restore", "purge"] = "trash",
    ) -> None:
        from nous_sdk.contracts.management import SourceSelection

        command = read_command(request, SourceSelection) if request else None
        invoke(
            ctx,
            lambda api: api.preview_sources(
                namespace_id,
                tuple(observations or ()),
                selection=command,
                action=action,
            ),
        )

    @source.command("operations")
    def source_operations(
        ctx: typer.Context,
        namespace_id: str,
        limit: int = 100,
        before_commit_seq: int | None = None,
    ) -> None:
        invoke(
            ctx,
            lambda api: api.source_operations(
                namespace_id, limit=limit, before_commit_seq=before_commit_seq
            ),
        )

    @source.command("operation")
    def source_operation(ctx: typer.Context, namespace_id: str, operation_id: str) -> None:
        invoke(
            ctx,
            lambda api: api.source_operation(namespace_id, source_operation_id=operation_id),
        )

    @source.command("manage")
    def manage_selection(
        ctx: typer.Context,
        namespace_id: str,
        action: Literal["trash", "restore", "purge"],
        request: Path,
    ) -> None:
        command = read_command(request, SourceCommand)
        if action == "trash":
            invoke(ctx, lambda api: api.trash_sources(namespace_id, command))
        elif action == "restore":
            invoke(ctx, lambda api: api.restore_sources(namespace_id, command))
        else:
            invoke(ctx, lambda api: api.purge_sources(namespace_id, command))

    @source.command("trash")
    def trash(
        ctx: typer.Context,
        namespace_id: str,
        observations: list[str],
        reason: Annotated[str, typer.Option(help="Reason for changing the selected sources.")],
        preview: str | None = None,
        operation_id: str | None = None,
        background: bool = False,
    ) -> None:
        payload = SourceCommand(
            operation_id=operation_id or str(uuid4()),
            observation_ids=tuple(observations),
            reason=reason,
            expected_preview=preview,
        )
        if background:

            async def submit(api: NousClient) -> dict[str, str]:
                return {"job_id": await api.submit_source_trash(namespace_id, payload)}

            invoke(ctx, submit)
        else:
            invoke(ctx, lambda api: api.trash_sources(namespace_id, payload))

    @source.command("restore")
    def restore(
        ctx: typer.Context,
        namespace_id: str,
        observations: list[str],
        reason: Annotated[str, typer.Option(help="Reason for changing the selected sources.")],
        operation_id: str | None = None,
    ) -> None:
        payload = SourceCommand(
            operation_id=operation_id or str(uuid4()),
            observation_ids=tuple(observations),
            reason=reason,
        )
        invoke(ctx, lambda api: api.restore_sources(namespace_id, payload))

    @source.command("purge")
    def purge(
        ctx: typer.Context,
        namespace_id: str,
        observations: list[str],
        reason: Annotated[str, typer.Option(help="Reason for changing the selected sources.")],
        preview: str | None = None,
        operation_id: str | None = None,
    ) -> None:
        payload = SourceCommand(
            operation_id=operation_id or str(uuid4()),
            observation_ids=tuple(observations),
            reason=reason,
            expected_preview=preview,
        )
        invoke(ctx, lambda api: api.purge_sources(namespace_id, payload))

    @source.command("download")
    def download(ctx: typer.Context, namespace_id: str, material_id: str, output: Path) -> None:
        invoke(
            ctx,
            lambda api: api.download_material(material_id, output, namespace_id=namespace_id),
        )

    @catalog.command("freeze")
    def freeze(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        payload = read_command(command, FreezeCatalog)
        invoke(ctx, lambda api: api.freeze_catalog(namespace_id, payload))

    @catalog.command("get")
    def get_catalog(ctx: typer.Context, namespace_id: str, catalog_id: str) -> None:
        invoke(ctx, lambda api: api.import_catalog(namespace_id, catalog_id))

    @catalog.command("process")
    def process(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        payload = read_command(command, ProcessCatalog)
        invoke(ctx, lambda api: api.submit_catalog(namespace_id, payload))

    @cognition.command("adopt")
    def adopt(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        payload = read_command(command, AdoptCognition)
        invoke(ctx, lambda api: api.adopt_cognition(namespace_id, payload))

    @cognition.command("list")
    def list_cognition(
        ctx: typer.Context,
        namespace_id: str,
        request: Path | None = None,
        cursor: str | None = None,
        limit: int = 100,
        history: bool = False,
        management: bool = False,
    ) -> None:
        """Page complete records; pass the returned cursor to continue the same snapshot."""
        command = (
            read_command(request, RecordsQuery)
            if request
            else RecordsQuery(
                cursor=cursor,
                limit=limit,
                view="history" if history else "current",
                purpose="management" if management else "current",
            )
        )
        invoke(ctx, lambda api: api.records(namespace_id, command))

    @cognition.command("read-batch")
    def read_cognition_batch(ctx: typer.Context, namespace_id: str, request: Path) -> None:
        """Read exact references from a ReadRecords JSON request."""
        command = read_command(request, ReadRecords)
        invoke(ctx, lambda api: api.read_records(namespace_id, command))

    @cognition.command("detail")
    def cognition_detail(
        ctx: typer.Context,
        namespace_id: str,
        reference: str,
        history_limit: int = 100,
        management: bool = False,
    ) -> None:
        """Read one exact cognitive object with its available revision history."""
        from nous_sdk.contracts.context import CognitiveTarget

        async def read(api: NousClient) -> object:
            resolved = await api.resolve_reference(namespace_id, reference)
            if not isinstance(resolved.reference, CognitiveTarget):
                raise ValueError("Reference must select a cognitive object")
            return await api.record_detail(
                namespace_id,
                resolved.reference.reference,
                history_limit=history_limit,
                purpose="management" if management else "exact",
            )

        invoke(ctx, read)

    @cognition.command("get")
    def get_cognition(ctx: typer.Context, namespace_id: str, reference: str) -> None:
        invoke(ctx, lambda api: api.resolve_reference(namespace_id, reference))

    @cognition.command("change-memory")
    def change_memory(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        value = read_command(command, MemoryChange)
        invoke(ctx, lambda api: api.update_memory(namespace_id, value))

    @cognition.command("merge-concepts")
    def merge(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        payload = read_command(command, MergeConcepts)
        invoke(ctx, lambda api: api.merge_concepts(namespace_id, payload))

    @cognition.command("split-concept")
    def split(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        payload = read_command(command, SplitConcept)
        invoke(ctx, lambda api: api.split_concept(namespace_id, payload))

    @cognition.command("resegment-episodes")
    def resegment(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        payload = read_command(command, ResegmentEpisodes)
        invoke(ctx, lambda api: api.resegment_episodes(namespace_id, payload))

    @bundle.command("export")
    def export(ctx: typer.Context, namespace_id: str, output: Path) -> None:
        output = output.resolve()

        async def run(api: NousClient) -> object:
            value = await api.export_bundle(namespace_id, BundleRequest())
            with output.open("x", encoding="utf-8") as target:
                target.write(json.dumps(value, ensure_ascii=False))
            return {"output": str(output)}

        invoke(ctx, run)

    @bundle.command("install")
    def install(
        ctx: typer.Context,
        namespace_id: str,
        archive: Path,
        operation_id: str | None = None,
    ) -> None:
        value = TypeAdapter(dict[str, JsonValue]).validate_python(read_json(archive))
        payload = InstallBundle(operation_id=operation_id or str(uuid4()), bundle=value)
        invoke(ctx, lambda api: api.install_bundle(namespace_id, payload))

    @media.command("upload")
    def upload(
        ctx: typer.Context,
        namespace_id: str,
        path: Path,
        media_type: str | None = None,
    ) -> None:
        mime = media_type or mimetypes.guess_type(path.name)[0] or "application/octet-stream"
        invoke(
            ctx,
            lambda api: api.upload_file(namespace_id, path, media_type=mime),
        )

    @media.command("adopt")
    def adopt_media(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        payload = read_command(command, AdoptMedia)
        invoke(ctx, lambda api: api.adopt_media(namespace_id, payload))

    @media.command("embed")
    def embed_media(
        ctx: typer.Context,
        namespace_id: str,
        reference: Path,
        operation_id: str | None = None,
    ) -> None:
        value = read_command(reference, MediaReference)
        invoke(
            ctx,
            lambda api: api.embed_media(namespace_id, value, operation_id=operation_id),
        )

    @media.command("register-region")
    def register_region(ctx: typer.Context, namespace_id: str, command: Path) -> None:
        value = read_command(command, SourceRegion)
        invoke(
            ctx,
            lambda api: api.request(
                "POST",
                f"/api/v1/namespaces/{segment(namespace_id)}/regions",
                body=encoded(value),
            ),
        )

    @media.command("region")
    def region(ctx: typer.Context, namespace_id: str, region_id: str) -> None:
        invoke(
            ctx,
            lambda api: api.request(
                "GET",
                f"/api/v1/namespaces/{segment(namespace_id)}/regions/{segment(region_id)}",
            ),
        )

    @media.command("download-region")
    def download_region(
        ctx: typer.Context, namespace_id: str, region_id: str, output: Path
    ) -> None:
        output = output.resolve()
        invoke(
            ctx,
            lambda api: api.download_region(namespace_id, region_id, output),
        )
