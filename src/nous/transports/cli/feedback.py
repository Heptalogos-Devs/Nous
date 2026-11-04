"""Feedback and explicit corrections for written recognition."""

from pathlib import Path
from typing import Annotated

import typer
from pydantic import JsonValue, TypeAdapter

from nous.cognition.feedback import Category, Value
from nous.cognition.retrieval import Mode
from nous.common.errors import NousError
from nous.content.contracts import ContentSelection
from nous.runtime.bootstrap import open_instance

from . import cli, emit, options, run_async
from .cognition import emit_record


@cli.command("feedback")
def feedback(
    ctx: typer.Context,
    ref: Annotated[str | None, typer.Argument()] = None,
    category: Category = "accuracy",
    value: Value | None = None,
    comment: str | None = None,
    author: str | None = None,
    namespace: str | None = None,
    snapshot: str | None = None,
    candidate: str | None = None,
    selection: Annotated[
        Path | None, typer.Option("--selection", exists=True, dir_okay=False)
    ] = None,
    query_result: Annotated[
        Path | None, typer.Option("--query-result", exists=True, dir_okay=False)
    ] = None,
    replace_file: Annotated[
        Path | None, typer.Option("--replace-file", exists=True, dir_okay=False)
    ] = None,
    expected: str | None = None,
    reason: str | None = None,
    refresh: bool = False,
    refresh_mode: Mode | None = None,
) -> None:
    if (refresh or refresh_mode is not None) and replace_file is None:
        raise NousError(
            "input.refresh_requires_correction", "Refresh accompanies a replacement correction"
        )
    if query_result is not None and (ref is not None or replace_file is not None):
        raise NousError("input.invalid_feedback", "Choose a record or a query result")
    if query_result is None and ref is None:
        raise NousError("input.invalid_feedback", "Provide a record or --query-result")

    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            if query_result is not None:
                context = TypeAdapter(dict[str, JsonValue]).validate_json(query_result.read_bytes())
                return await runtime.feedback.record_query(
                    context,
                    category,
                    value=value,
                    comment=comment,
                    author=author,
                    namespace=namespace,
                )
            assert ref is not None
            if replace_file is not None:
                base = expected or (ref if "@" in ref else None)
                if base is None:
                    raise NousError(
                        "input.expected_required", "Provide an exact revision or --expected"
                    )
                if base.isdecimal():
                    base = ref.split("@")[0] + "@" + base
                if not reason:
                    raise NousError("input.reason_required", "A correction requires --reason")
                return await runtime.feedback.correct(
                    ref,
                    base,
                    replace_file.read_text(encoding="utf-8"),
                    reason=reason,
                    author=author,
                    namespace=namespace,
                    refresh=refresh,
                    refresh_mode=refresh_mode,
                )
            return await runtime.feedback.record(
                ref,
                category,
                value=value,
                comment=comment,
                author=author,
                namespace=namespace,
                snapshot=snapshot,
                candidate=candidate,
                selection=ContentSelection.model_validate_json(selection.read_bytes())
                if selection is not None
                else None,
            )

    result = run_async(run())
    if isinstance(result, tuple):
        opinion, record = result
        if options(ctx).json_mode:
            emit(
                ctx,
                {
                    "feedback": opinion.model_dump(mode="json"),
                    "record": record.model_dump(mode="json"),
                },
            )
        else:
            emit_record(ctx, record)
            typer.echo(f"已记录更正 / Correction recorded: {opinion.address}")
    elif options(ctx).json_mode:
        emit(ctx, result.model_dump(mode="json"))
    else:
        typer.echo(f"已记录反馈 / Feedback recorded: {result.address}")
