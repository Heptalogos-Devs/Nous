"""User-facing written record commands call the shared cognitive owners."""

import json
import sys
from pathlib import Path
from typing import Annotated

import typer

from nous.cognition.contracts import RecordView, SavedRecord, SourceBinding
from nous.cognition.records import WrittenRecords
from nous.cognition.retrieval import Mode
from nous.common.errors import NousError
from nous.runtime.bootstrap import open_instance

from . import cli, options, run_async

note_cli = typer.Typer(help="记录笔记 / Notes", no_args_is_help=True)
journal_cli = typer.Typer(help="长期文稿 / Journals", no_args_is_help=True)
namespace_cli = typer.Typer(help="认知范围 / Namespaces", no_args_is_help=True)
cli.add_typer(note_cli, name="note")
cli.add_typer(journal_cli, name="journal")
cli.add_typer(namespace_cli, name="namespace")


def read_body(text: str | None, file: Path | None) -> str:
    stdin = not sys.stdin.isatty()
    if text is not None and file is not None:
        raise NousError("input.conflicting_body", "Choose one of --text, --file or stdin")
    if stdin and (text is not None or file is not None) and sys.stdin.read():
        raise NousError("input.conflicting_body", "Choose one of --text, --file or stdin")
    if text is not None:
        return text
    if file is not None:
        return file.read_text(encoding="utf-8")
    if stdin:
        return sys.stdin.read()
    raise NousError("input.body_required", "Provide --text, --file or stdin")


def emit_record(ctx: typer.Context, result: RecordView | SavedRecord) -> None:
    if options(ctx).json_mode:
        typer.echo(result.model_dump_json())
        return
    value = result.model_dump(mode="json")
    revision = getattr(result, "revision_ref", None)
    if revision:
        typer.echo(f"{value['title']} — {revision.address}")
    if isinstance(result, SavedRecord):
        if result.annotation == "adopted":
            typer.echo(f"标注 / Annotation: {result.annotation_ref}")
        elif result.annotation == "failed" and result.annotation_error:
            typer.echo(
                f"正文已保存；标注失败 / Body saved; annotation failed: {result.annotation_error.get('message')}"
            )
    if isinstance(result, RecordView):
        typer.echo(value["body"])
        if result.current_revision != result.revision_ref:
            typer.echo(f"当前版本 / Current: {result.current_revision.address}")
        for key, body in value.get("source_texts", {}).items():
            typer.echo(f"\n[{key}]\n{body}")
    elif "saved" in value:
        if result.retrieval_state == "failed" and result.retrieval_error:
            error = result.retrieval_error
            typer.echo(
                f"正文已保存；检索刷新失败 / Body saved; retrieval refresh failed: {error.get('message')}"
            )
            details = error.get("details")
            if isinstance(details, dict) and details.get("retry"):
                typer.echo(str(details["retry"]))
            if error.get("operation_id"):
                typer.echo(f"Operation: {error['operation_id']}")
        else:
            typer.echo(
                "已保存；检索已更新 / Saved; retrieval updated"
                if result.retrieval_state == "ready"
                else "已保存；检索将在下次查询时更新 / Saved; retrieval updates on next query"
                if result.retrieval_state == "pending"
                else "已保存 / Saved"
            )


async def select_source_bindings(
    records: WrittenRecords,
    namespace: str | None,
    source: list[str] | None,
    current_basis: list[str] | None,
    inspiration: list[str] | None,
) -> tuple[SourceBinding, ...]:
    references, bases, ideas = source or [], current_basis or [], inspiration or []
    bindings = await records.select_sources(namespace, [*references, *bases, *ideas])
    return tuple(
        binding.model_copy(
            update={
                "usage": "reference"
                if index < len(references)
                else "current_basis"
                if index < len(references) + len(bases)
                else "inspiration"
            }
        )
        for index, binding in enumerate(bindings)
    )


async def _create(
    ctx: typer.Context,
    namespace: str | None,
    text: str | None,
    file: Path | None,
    title: str | None,
    kind: str,
    remember: bool | None = None,
    purpose: str = "record",
    source: list[str] | None = None,
    current_basis: list[str] | None = None,
    inspiration: list[str] | None = None,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
) -> SavedRecord:
    body = read_body(text, file)
    async with open_instance(options(ctx).locator) as runtime:
        bindings = await select_source_bindings(
            runtime.records, namespace, source, current_basis, inspiration
        )
        if kind == "note":
            return await runtime.records.record_note(
                namespace,
                body,
                title=title,
                remember=remember,
                sources=bindings,
                refresh=refresh,
                refresh_mode=refresh_mode,
            )
        return await runtime.records.create_journal(
            namespace,
            body,
            title=title,
            purpose=purpose,
            sources=bindings,
            refresh=refresh,
            refresh_mode=refresh_mode,
        )


@note_cli.command("add")
def note_add(
    ctx: typer.Context,
    text: str | None = None,
    file: Path | None = None,
    title: str | None = None,
    remember: Annotated[bool | None, typer.Option("--remember/--no-remember")] = None,
    namespace: str | None = None,
    source: Annotated[
        list[str] | None,
        typer.Option("--source", help="Fixed source record; repeated inputs use S1, S2..."),
    ] = None,
    current_basis: Annotated[list[str] | None, typer.Option("--current-basis")] = None,
    inspiration: Annotated[list[str] | None, typer.Option("--inspiration")] = None,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
):
    emit_record(
        ctx,
        run_async(
            _create(
                ctx,
                namespace,
                text,
                file,
                title,
                "note",
                remember,
                source=source,
                current_basis=current_basis,
                inspiration=inspiration,
                refresh=refresh,
                refresh_mode=refresh_mode,
            )
        ),
    )


@journal_cli.command("create")
def journal_create(
    ctx: typer.Context,
    text: str | None = None,
    file: Path | None = None,
    title: str | None = None,
    purpose: str = "record",
    namespace: str | None = None,
    source: Annotated[
        list[str] | None,
        typer.Option("--source", help="Fixed source record; repeated inputs use S1, S2..."),
    ] = None,
    current_basis: Annotated[list[str] | None, typer.Option("--current-basis")] = None,
    inspiration: Annotated[list[str] | None, typer.Option("--inspiration")] = None,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
):
    emit_record(
        ctx,
        run_async(
            _create(
                ctx,
                namespace,
                text,
                file,
                title,
                "journal",
                purpose=purpose,
                source=source,
                current_basis=current_basis,
                inspiration=inspiration,
                refresh=refresh,
                refresh_mode=refresh_mode,
            )
        ),
    )


@cli.command("read")
def read_record(
    ctx: typer.Context,
    ref: str,
    namespace: str | None = None,
    sources: bool = False,
    blocks: str | None = None,
):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await runtime.records.read(
                ref,
                namespace=namespace,
                sources=sources,
                blocks=tuple(blocks.split(",")) if blocks else (),
            )

    emit_record(ctx, run_async(run()))


@journal_cli.command("revise")
@note_cli.command("revise")
def revise_record(
    ctx: typer.Context,
    ref: str,
    text: str | None = None,
    file: Path | None = None,
    expected: str | None = None,
    title: str | None = None,
    reason: str | None = None,
    namespace: str | None = None,
    source: Annotated[list[str] | None, typer.Option("--source")] = None,
    clear_sources: bool = False,
    current_basis: Annotated[list[str] | None, typer.Option("--current-basis")] = None,
    inspiration: Annotated[list[str] | None, typer.Option("--inspiration")] = None,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
):
    if (source or current_basis or inspiration) and clear_sources:
        raise NousError("input.conflicting_sources", "Choose --source or --clear-sources")
    if expected is None:
        if "@" not in ref:
            raise NousError("input.expected_required", "Provide an exact ref with @N or --expected")
        expected = ref
    elif expected.isdecimal():
        expected = ref.split("@")[0] + "@" + expected
    body = read_body(text, file)

    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            bindings = (
                await select_source_bindings(
                    runtime.records, namespace, source, current_basis, inspiration
                )
                if source is not None or current_basis is not None or inspiration is not None
                else (() if clear_sources else None)
            )
            return await runtime.records.revise(
                ref,
                expected,
                body,
                namespace=namespace,
                title=title,
                reason=reason,
                sources=bindings,
                refresh=refresh,
                refresh_mode=refresh_mode,
            )

    emit_record(ctx, run_async(run()))


@journal_cli.command("edit")
@note_cli.command("edit")
def edit_record(
    ctx: typer.Context,
    ref: str,
    namespace: str | None = None,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
):
    async def load():
        async with open_instance(options(ctx).locator) as runtime:
            return await runtime.records.read(ref, namespace=namespace)

    original = run_async(load())
    import click

    edited = click.edit(original.body, extension=".md", require_save=True)
    if edited is None:
        return

    async def save():
        async with open_instance(options(ctx).locator) as runtime:
            try:
                return await runtime.records.revise(
                    original.ref,
                    original.revision_ref,
                    edited,
                    namespace=namespace,
                    refresh=refresh,
                    refresh_mode=refresh_mode,
                )
            except NousError as error:
                if error.code == "cognition.revision_conflict":
                    from uuid import uuid4

                    path = runtime.paths.temp / ("edit-" + str(uuid4()) + ".md")
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(edited, encoding="utf-8")
                    error.details["edited_file"] = str(path)
                raise

    emit_record(ctx, run_async(save()))


@journal_cli.command("history")
@note_cli.command("history")
def history(ctx: typer.Context, ref: str, namespace: str | None = None):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await runtime.records.history(ref, namespace=namespace)

    result = run_async(run())
    if options(ctx).json_mode:
        typer.echo(
            json.dumps(
                {"revisions": [item.model_dump(mode="json") for item in result]}, ensure_ascii=False
            )
        )
    else:
        for item in result:
            typer.echo(f"{item.revision_ref.address}  {item.title}  {item.recorded_at.isoformat()}")


async def _membership(
    ctx: typer.Context,
    ref: str,
    namespace: str | None,
    operation: str,
    expected: str | None = None,
    enabled: bool | None = None,
) -> SavedRecord:
    if expected is None and "@" not in ref:
        raise NousError("input.expected_required", "Provide an exact ref with @N or --expected")
    expected = expected or ref
    if expected.isdecimal():
        expected = ref.split("@")[0] + "@" + expected
    async with open_instance(options(ctx).locator) as runtime:
        if operation == "recall":
            return await runtime.records.set_recall(
                ref, expected, bool(enabled), namespace=namespace
            )
        return await getattr(runtime.records, operation)(ref, expected, namespace=namespace)


@journal_cli.command("archive")
@note_cli.command("archive")
def archive(
    ctx: typer.Context, ref: str, namespace: str | None = None, expected: str | None = None
):
    emit_record(ctx, run_async(_membership(ctx, ref, namespace, "archive", expected)))


@journal_cli.command("restore")
@note_cli.command("restore")
def restore(
    ctx: typer.Context, ref: str, namespace: str | None = None, expected: str | None = None
):
    emit_record(ctx, run_async(_membership(ctx, ref, namespace, "restore", expected)))


@note_cli.command("remember")
def remember(
    ctx: typer.Context,
    ref: str,
    namespace: str | None = None,
    expected: str | None = None,
    enabled: bool = True,
):
    emit_record(ctx, run_async(_membership(ctx, ref, namespace, "recall", expected, enabled)))


@journal_cli.command("list")
@note_cli.command("list")
def list_records(
    ctx: typer.Context,
    namespace: str | None = None,
    archived: bool = False,
    offset: int = 0,
    limit: int | None = None,
):
    kind = "note" if ctx.parent is not None and ctx.parent.info_name == "note" else "journal"

    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await runtime.records.list(
                namespace, kind=kind, archived=archived, offset=offset, limit=limit
            )

    result = run_async(run())
    if options(ctx).json_mode:
        typer.echo(
            json.dumps(
                {"records": [item.model_dump(mode="json") for item in result]}, ensure_ascii=False
            )
        )
    else:
        for item in result:
            typer.echo(f"{item.revision_ref.address}  {item.title}")


@namespace_cli.command("create")
def namespace_create(ctx: typer.Context, name: str):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await runtime.namespaces.create(name)

    result = run_async(run())
    typer.echo(result.model_dump_json() if options(ctx).json_mode else result.name)


@namespace_cli.command("list")
def namespace_list(ctx: typer.Context):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await runtime.namespaces.list()

    result = run_async(run())
    typer.echo(
        json.dumps(
            {"namespaces": [item.model_dump(mode="json") for item in result]}, ensure_ascii=False
        )
        if options(ctx).json_mode
        else "\n".join(item.name for item in result)
    )


@namespace_cli.command("rename")
def namespace_rename(ctx: typer.Context, namespace: str, name: str):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await runtime.namespaces.rename(namespace, name)

    result = run_async(run())
    typer.echo(result.model_dump_json() if options(ctx).json_mode else result.name)


@namespace_cli.command("select")
def namespace_select(ctx: typer.Context, namespace: str):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await runtime.namespaces.select(namespace)

    result = run_async(run())
    typer.echo(result.model_dump_json() if options(ctx).json_mode else result.name)
