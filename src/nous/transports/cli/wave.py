"""Build, query and inspect Wave collections using shared operations."""

import asyncio
from collections.abc import Mapping
from pathlib import Path
from typing import Annotated, Literal

import typer

from nous.application.inputs import FileInput, InputProvider, StreamInput
from nous.common.errors import NousError
from nous.content.contracts import ExternalInput, SourceSelection
from nous.models.vector_contracts import VectorRef
from nous.storage.file_input import FileReaderPool
from nous.wave.contracts import (
    CandidateFilter,
    CandidatePool,
    CollectionManifest,
    ContextFragment,
    PassageDelivery,
    QueryCue,
    ThinkingStage,
    WaveBuild,
    WaveCleanup,
    WaveQuery,
    WaveRead,
    WaveSearchResult,
    WaveSegment,
    WaveSnapshotRequest,
)
from nous.wave.operations import (
    WAVE_BUILD,
    WAVE_CLEANUP,
    WAVE_INSPECT,
    WAVE_QUERY,
    WAVE_READ,
    WAVE_SEGMENT,
)

from .context import application_call, emit, options
from .input_files import json_binding

wave_cli = typer.Typer(help="构建、检索和回读内容集合 / Wave retrieval", no_args_is_help=True)


@wave_cli.command("cleanup")
def cleanup(
    ctx: typer.Context,
    keep_recent: Annotated[int, typer.Option("--keep-recent", min=0)] = 0,
    retain: Annotated[list[str] | None, typer.Option("--retain")] = None,
) -> None:
    result = application_call(
        ctx, WAVE_CLEANUP, WaveCleanup(keep_recent=keep_recent, retain=tuple(retain or ()))
    )
    emit(ctx, result.model_dump(mode="json"))


def _show_passage(value: PassageDelivery) -> None:
    if value.reason == "title_preview":
        typer.echo("标题命中 / Title match")
    typer.echo(value.text)
    for fragment in value.fragments:
        typer.echo(
            f"   {fragment.content.artifact.identifier} {fragment.content.selection.model_dump_json()}"
        )


def _hits(value: WaveSearchResult, explain: bool) -> None:
    groups = value.groups
    if groups:
        for group in groups:
            typer.echo(
                f"{group.rank}. {group.group.title or group.group.key}  {group.basis.score_kind}={group.basis.ranking_score}"
            )
            for passage in group.passages:
                _show_passage(passage)
            if explain:
                typer.echo(group.model_dump_json(indent=2))
    else:
        hits = value.hits
        for hit in hits:
            basis = hit.basis
            passage = hit.passage
            typer.echo(
                f"{basis.candidate_rank}. {basis.candidate.key}  {basis.score_kind}={basis.ranking_score}"
            )
            _show_passage(passage)
            if explain:
                typer.echo(basis.model_dump_json(indent=2))
        if not hits:
            typer.echo("No matches / 没有匹配内容")


@wave_cli.command("build")
def build(
    ctx: typer.Context,
    manifest: Annotated[Path, typer.Option("--manifest")],
    space: Annotated[str | None, typer.Option("--space")] = None,
) -> None:
    async def prepare(
        request: WaveBuild, bindings: Mapping[str, InputProvider], limit: int
    ) -> tuple[WaveBuild, Mapping[str, InputProvider]]:
        try:
            resolved_manifest = await asyncio.to_thread(manifest.resolve, strict=True)
        except OSError as error:
            raise NousError(
                "input.unreadable",
                "Cannot read external input",
                details={"source": str(manifest)},
                cause=error,
            ) from error
        readers = FileReaderPool(1)
        chunks = readers.chunks(
            resolved_manifest, 1024 * 1024, selection=SourceSelection(), limit=limit
        )
        try:
            data = b"".join([chunk async for chunk in chunks])
        except TimeoutError:
            raise
        except OSError as error:
            raise NousError(
                "input.unreadable",
                "Cannot read external input",
                details={"source": str(manifest)},
                cause=error,
            ) from error
        finally:
            await chunks.aclose()
            await readers.aclose()
        declaration = CollectionManifest.model_validate_json(data)
        inputs: dict[str, InputProvider] = {
            **bindings,
            "manifest": StreamInput((data,), name=manifest.name, source=str(manifest)),
        }
        documents: dict[str, ExternalInput] = {}
        for index, document in enumerate(declaration.documents, 1):
            if document.input.file is None:
                continue
            path = Path(document.input.file)
            if not path.is_absolute():
                path = resolved_manifest.parent / path
            key = f"document_{index}"
            inputs[key] = FileInput(path, name=path.name, source=str(path))
            documents[document.key] = ExternalInput(key=key)
        return WaveBuild(
            manifest=request.manifest, documents=documents, space=request.space
        ), inputs

    result = application_call(
        ctx,
        WAVE_BUILD,
        WaveBuild(manifest=ExternalInput(key="manifest"), documents={}, space=space),
        prepare=prepare,
    )
    emit(ctx, result.model_dump(mode="json"))


@wave_cli.command("inspect")
def inspect(
    ctx: typer.Context,
    snapshot: str,
    concept: Annotated[list[str] | None, typer.Option("--concept")] = None,
) -> None:
    emit(
        ctx,
        application_call(
            ctx, WAVE_INSPECT, WaveSnapshotRequest(snapshot=snapshot, concepts=tuple(concept or ()))
        ).model_dump(mode="json"),
    )


@wave_cli.command("query")
def query(
    ctx: typer.Context,
    snapshot: str,
    text: Annotated[str, typer.Argument()] = "",
    mode: Annotated[
        Literal["lexical", "dense", "tagmemo", "rivermemo", "hybrid", "dtsc"] | None,
        typer.Option("--mode"),
    ] = None,
    context: Annotated[Path | None, typer.Option("--context", exists=True, dir_okay=False)] = None,
    core: Annotated[list[str] | None, typer.Option("--core")] = None,
    cue: Annotated[list[str] | None, typer.Option("--cue")] = None,
    thinking_stages: Annotated[
        Path | None, typer.Option("--thinking-stages", exists=True, dir_okay=False)
    ] = None,
    candidate_filter: Annotated[
        Path | None, typer.Option("--filter", exists=True, dir_okay=False)
    ] = None,
    vector_ref: Annotated[
        Path | None, typer.Option("--vector-ref", exists=True, dir_okay=False)
    ] = None,
    candidate_pool: Annotated[
        Path | None, typer.Option("--candidate-pool", exists=True, dir_okay=False)
    ] = None,
    top_k: Annotated[int | None, typer.Option("--top-k", min=1)] = None,
    grouping: Annotated[Literal["candidate", "document"], typer.Option("--grouping")] = "candidate",
    passages_per_group: Annotated[int, typer.Option("--passages-per-group", min=1)] = 1,
    explain: Annotated[bool, typer.Option("--explain")] = False,
) -> None:
    inputs: dict[str, InputProvider] = {}

    def source(path: Path | None, key: str) -> ExternalInput | None:
        if path is None:
            return None
        value, binding = json_binding(path, key=key)
        inputs.update(binding)
        return value

    query_context: tuple[ContextFragment, ...] | ExternalInput = source(context, "context") or ()
    stages: tuple[ThinkingStage, ...] | ExternalInput = (
        source(thinking_stages, "thinking_stages") or ()
    )
    vector: VectorRef | ExternalInput | None = source(vector_ref, "vector")
    pool: CandidatePool | ExternalInput | None = source(candidate_pool, "candidate_pool")
    filters: CandidateFilter | ExternalInput | None = source(candidate_filter, "candidate_filter")
    result = application_call(
        ctx,
        WAVE_QUERY,
        WaveQuery(
            snapshot=snapshot,
            text=text,
            mode=mode,
            context=query_context,
            vector=vector,
            candidate_pool=pool,
            candidate_filter=filters,
            thinking_stages=stages,
            core_concepts=tuple(core or ()),
            cues=tuple(QueryCue(label=item, description=item) for item in cue or ()),
            top_k=top_k,
            grouping=grouping,
            passages_per_group=passages_per_group,
            include_trace=explain,
        ),
        inputs=inputs,
    )
    if options(ctx).json_mode:
        emit(ctx, result.model_dump(mode="json"))
    else:
        _hits(result, explain)


@wave_cli.command("read")
def read(
    ctx: typer.Context,
    snapshot: str,
    candidate: str,
    sources: Annotated[bool, typer.Option("--sources")] = False,
) -> None:
    result = application_call(
        ctx, WAVE_READ, WaveRead(snapshot=snapshot, candidate=candidate, sources=sources)
    )
    if options(ctx).json_mode:
        emit(ctx, result.model_dump(mode="json"))
    else:
        typer.echo(result.text)
        if sources:
            emit(ctx, result.model_dump(mode="json", exclude={"text"}))


@wave_cli.command("segment")
def segment(
    ctx: typer.Context,
    snapshot: str,
    context: Annotated[Path, typer.Argument(exists=True, dir_okay=False)],
) -> None:
    fragments, inputs = json_binding(context, key="fragments")
    emit(
        ctx,
        application_call(
            ctx, WAVE_SEGMENT, WaveSegment(snapshot=snapshot, fragments=fragments), inputs=inputs
        ).model_dump(mode="json"),
    )
