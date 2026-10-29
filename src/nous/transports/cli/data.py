"""Identity, import, retrieval and source lifecycle commands."""

import json
import mimetypes
from pathlib import Path
from typing import Annotated, Literal
from uuid import uuid4

import typer
from nous_wave import NousClient
from nous_wave.contracts import (
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
from nous_wave.contracts.lifecycle import MemoryChange
from nous_wave.operations import encoded, segment
from pydantic import JsonValue, TypeAdapter

from .common import invoke, read_command, read_json


def register(cli: typer.Typer) -> None:
    namespace = typer.Typer(help="Namespace directory and creation.", no_args_is_help=False)
    subject = typer.Typer(help="Subject ownership and capabilities.", no_args_is_help=False)
    source = typer.Typer(help="Original sources and their lifecycle.", no_args_is_help=False)
    catalog = typer.Typer(help="Freeze and process input catalogs.", no_args_is_help=False)
    cognition = typer.Typer(help="Exact revisions and cognitive evolution.", no_args_is_help=False)
    bundle = typer.Typer(help="Logical subject export and install.", no_args_is_help=False)
    media = typer.Typer(help="Upload and adopt original media.", no_args_is_help=False)
    for name, group in (
        ("namespace", namespace),
        ("subject", subject),
        ("source", source),
        ("catalog", catalog),
        ("cognition", cognition),
        ("bundle", bundle),
        ("media", media),
    ):
        cli.add_typer(group, name=name)

    @namespace.command("list")
    def namespaces(ctx: typer.Context) -> None:
        invoke(ctx, lambda api: api.namespaces())

    @namespace.command("create")
    def create_namespace(ctx: typer.Context, name: str, operation_id: str | None = None) -> None:
        invoke(ctx, lambda api: api.create_namespace(name, operation_id=operation_id))

    @subject.command("list")
    def subjects(ctx: typer.Context, namespace_id: str) -> None:
        invoke(ctx, lambda api: api.subjects(namespace_id))

    @subject.command("create")
    def create_subject(
        ctx: typer.Context,
        namespace_id: str,
        name: str = "default",
        capability: list[str] | None = None,
        operation_id: str | None = None,
    ) -> None:
        invoke(
            ctx,
            lambda api: api.create_subject(
                namespace_id,
                name=name,
                capabilities=capability or ("memory",),
                operation_id=operation_id,
            ),
        )

    @subject.command("get")
    def get_subject(ctx: typer.Context, subject_id: str) -> None:
        invoke(ctx, lambda api: api.subject(subject_id))

    @subject.command("diagnostics")
    def diagnostics(ctx: typer.Context, subject_id: str) -> None:
        invoke(ctx, lambda api: api.subject_diagnostics(subject_id))

    @cli.command("import")
    def import_documents(
        ctx: typer.Context,
        namespace_id: str,
        subject_id: str,
        paths: list[Path],
        llm: bool = True,
        vectors: Literal["disabled", "metadata_allowed", "required"] = "metadata_allowed",
        operation_id: str | None = None,
    ) -> None:
        """Adopt actual UTF-8 documents; --no-llm keeps literal text."""
        command = AdmitFiles(
            operation_id=operation_id or str(uuid4()),
            llm_enabled=llm,
            vectors=vectors,
            inputs=tuple(
                ImportDocument(
                    text=path.read_text(encoding="utf-8"),
                    filename=path.name,
                    file_identity=str(path.resolve()),
                    source_type="document",
                )
                for path in paths
            ),
        )
        invoke(ctx, lambda api: api.admit_documents(namespace_id, subject_id, command))

    @cli.command("query")
    def query(
        ctx: typer.Context,
        namespace_id: str,
        subject_id: str,
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
        invoke(ctx, lambda api: api.query_cognition(namespace_id, subject_id, payload))

    @cli.command("manual")
    def manual(ctx: typer.Context) -> None:
        invoke(ctx, lambda api: api.query_manual())

    @cli.command("graph-search")
    def graph_search(
        ctx: typer.Context, namespace_id: str, subject_id: str, query: str, limit: int = 50
    ) -> None:
        """Search graph entities and relations using the A-Memorix search contract."""
        from nous_wave.amemorix import AMemorixAdapter

        async def search(api: NousClient) -> object:
            return await AMemorixAdapter(
                api, namespace_id=namespace_id, subject_id=subject_id
            ).memory_graph_admin(action="search", query=query, limit=limit)

        invoke(ctx, search)

    @cli.command("graph")
    def graph(
        ctx: typer.Context,
        namespace_id: str,
        subject_id: str,
        action: Annotated[str, typer.Argument()] = "get_graph",
        request: Path | None = None,
    ) -> None:
        """Read graph details or apply a source-backed manual graph action from JSON."""
        from nous_wave.amemorix import AMemorixAdapter

        values = (
            TypeAdapter(dict[str, JsonValue]).validate_python(read_json(request)) if request else {}
        )

        async def run(api: NousClient) -> object:
            return await AMemorixAdapter(
                api, namespace_id=namespace_id, subject_id=subject_id
            ).memory_graph_admin(action=action, **values)

        invoke(ctx, run)

    @source.command("list")
    def sources(ctx: typer.Context, namespace_id: str, subject_id: str) -> None:
        invoke(ctx, lambda api: api.source_directory(namespace_id, subject_id))

    @source.command("read")
    def read_source(ctx: typer.Context, subject_id: str, reference: str) -> None:
        invoke(ctx, lambda api: api.source(subject_id, reference))

    @source.command("preview")
    def preview(
        ctx: typer.Context, namespace_id: str, subject_id: str, observations: list[str]
    ) -> None:
        invoke(ctx, lambda api: api.preview_sources(namespace_id, subject_id, tuple(observations)))

    @source.command("trash")
    def trash(
        ctx: typer.Context,
        namespace_id: str,
        subject_id: str,
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
        invoke(ctx, lambda api: api.trash_sources(namespace_id, subject_id, payload))

    @source.command("restore")
    def restore(
        ctx: typer.Context,
        namespace_id: str,
        subject_id: str,
        observations: list[str],
        reason: Annotated[str, typer.Option(help="Reason for changing the selected sources.")],
        operation_id: str | None = None,
    ) -> None:
        payload = SourceCommand(
            operation_id=operation_id or str(uuid4()),
            observation_ids=tuple(observations),
            reason=reason,
        )
        invoke(ctx, lambda api: api.restore_sources(namespace_id, subject_id, payload))

    @source.command("purge")
    def purge(
        ctx: typer.Context,
        namespace_id: str,
        subject_id: str,
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
        invoke(ctx, lambda api: api.purge_sources(namespace_id, subject_id, payload))

    @source.command("download")
    def download(
        ctx: typer.Context, namespace_id: str, subject_id: str, material_id: str, output: Path
    ) -> None:
        invoke(
            ctx,
            lambda api: api.download_material(
                material_id, output, namespace_id=namespace_id, subject_id=subject_id
            ),
        )

    @catalog.command("freeze")
    def freeze(ctx: typer.Context, namespace_id: str, subject_id: str, command: Path) -> None:
        payload = read_command(command, FreezeCatalog)
        invoke(ctx, lambda api: api.freeze_catalog(namespace_id, subject_id, payload))

    @catalog.command("get")
    def get_catalog(
        ctx: typer.Context, namespace_id: str, subject_id: str, catalog_id: str
    ) -> None:
        invoke(ctx, lambda api: api.import_catalog(namespace_id, subject_id, catalog_id))

    @catalog.command("process")
    def process(ctx: typer.Context, namespace_id: str, subject_id: str, command: Path) -> None:
        payload = read_command(command, ProcessCatalog)
        invoke(ctx, lambda api: api.submit_catalog(namespace_id, subject_id, payload))

    @cognition.command("adopt")
    def adopt(ctx: typer.Context, namespace_id: str, subject_id: str, command: Path) -> None:
        payload = read_command(command, AdoptCognition)
        invoke(ctx, lambda api: api.adopt_cognition(namespace_id, subject_id, payload))

    @cognition.command("get")
    def get_cognition(ctx: typer.Context, subject_id: str, reference: str) -> None:
        invoke(ctx, lambda api: api.resolve_reference(subject_id, reference))

    @cognition.command("change-memory")
    def change_memory(
        ctx: typer.Context, namespace_id: str, subject_id: str, command: Path
    ) -> None:
        value = read_command(command, MemoryChange)
        invoke(ctx, lambda api: api.update_memory(namespace_id, subject_id, value))

    @cognition.command("merge-concepts")
    def merge(ctx: typer.Context, namespace_id: str, subject_id: str, command: Path) -> None:
        payload = read_command(command, MergeConcepts)
        invoke(ctx, lambda api: api.merge_concepts(namespace_id, subject_id, payload))

    @cognition.command("split-concept")
    def split(ctx: typer.Context, namespace_id: str, subject_id: str, command: Path) -> None:
        payload = read_command(command, SplitConcept)
        invoke(ctx, lambda api: api.split_concept(namespace_id, subject_id, payload))

    @cognition.command("resegment-episodes")
    def resegment(ctx: typer.Context, namespace_id: str, subject_id: str, command: Path) -> None:
        payload = read_command(command, ResegmentEpisodes)
        invoke(ctx, lambda api: api.resegment_episodes(namespace_id, subject_id, payload))

    @bundle.command("export")
    def export(ctx: typer.Context, namespace_id: str, subject_id: str, output: Path) -> None:
        output = output.resolve()

        async def run(api: NousClient) -> object:
            value = await api.export_bundle(namespace_id, subject_id, BundleRequest())
            with output.open("x", encoding="utf-8") as target:
                target.write(json.dumps(value, ensure_ascii=False))
            return {"output": str(output)}

        invoke(ctx, run)

    @bundle.command("install")
    def install(
        ctx: typer.Context,
        namespace_id: str,
        subject_id: str,
        archive: Path,
        operation_id: str | None = None,
    ) -> None:
        value = TypeAdapter(dict[str, JsonValue]).validate_python(read_json(archive))
        payload = InstallBundle(operation_id=operation_id or str(uuid4()), bundle=value)
        invoke(ctx, lambda api: api.install_bundle(namespace_id, subject_id, payload))

    @media.command("upload")
    def upload(
        ctx: typer.Context,
        namespace_id: str,
        subject_id: str,
        path: Path,
        media_type: str | None = None,
    ) -> None:
        mime = media_type or mimetypes.guess_type(path.name)[0] or "application/octet-stream"
        invoke(
            ctx,
            lambda api: api.upload_file(
                namespace_id, path, owner_subject_id=subject_id, media_type=mime
            ),
        )

    @media.command("adopt")
    def adopt_media(ctx: typer.Context, namespace_id: str, subject_id: str, command: Path) -> None:
        payload = read_command(command, AdoptMedia)
        invoke(ctx, lambda api: api.adopt_media(namespace_id, subject_id, payload))

    @media.command("embed")
    def embed_media(
        ctx: typer.Context,
        namespace_id: str,
        subject_id: str,
        reference: Path,
        operation_id: str | None = None,
    ) -> None:
        value = read_command(reference, MediaReference)
        invoke(
            ctx,
            lambda api: api.embed_media(namespace_id, subject_id, value, operation_id=operation_id),
        )

    @media.command("register-region")
    def register_region(
        ctx: typer.Context, namespace_id: str, subject_id: str, command: Path
    ) -> None:
        value = read_command(command, SourceRegion)
        invoke(
            ctx,
            lambda api: api.request(
                "POST",
                f"/api/namespaces/{segment(namespace_id)}/subjects/{segment(subject_id)}/regions",
                body=encoded(value),
            ),
        )

    @media.command("region")
    def region(ctx: typer.Context, namespace_id: str, subject_id: str, region_id: str) -> None:
        invoke(
            ctx,
            lambda api: api.request(
                "GET",
                f"/api/namespaces/{segment(namespace_id)}/subjects/{segment(subject_id)}/regions/{segment(region_id)}",
            ),
        )

    @media.command("download-region")
    def download_region(
        ctx: typer.Context, namespace_id: str, subject_id: str, region_id: str, output: Path
    ) -> None:
        output = output.resolve()
        invoke(
            ctx,
            lambda api: api.download_region(namespace_id, subject_id, region_id, output),
        )
