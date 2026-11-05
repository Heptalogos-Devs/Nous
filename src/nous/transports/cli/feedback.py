"""Record and resolve feedback through the cognitive application owner."""

from pathlib import Path
from typing import Annotated, Literal

import typer

from nous.cognition.feedback import Category, FeedbackView, Value
from nous.cognition.retrieval.contracts import QueryResult
from nous.common.errors import NousError
from nous.content.contracts import ContentSelection
from nous.presentation import translate
from nous.runtime.bootstrap import open_instance

from . import cli, emit, language_for, options, run_async

feedback_cli = typer.Typer(help="Feedback", no_args_is_help=True)
cli.add_typer(feedback_cli, name="feedback")


def _show(ctx: typer.Context, feedback: FeedbackView) -> None:
    if options(ctx).json_mode:
        emit(ctx, feedback.model_dump(mode="json"))
        return
    language = language_for(options(ctx))
    def tr(message: str) -> str:
        return translate(message, language)
    typer.echo(f"{tr('Feedback')}: {feedback.address}")
    if feedback.target.revision is not None:
        typer.echo(f"{tr('Target')}: {feedback.target.revision.address}")
    elif feedback.target.query is not None:
        typer.echo(f"{tr('Query')}: {feedback.target.query.context.request_text}")
    typer.echo(f"{tr('Category')}: {tr(feedback.category)}")
    if feedback.value is not None:
        typer.echo(f"{tr('Value')}: {tr(feedback.value)}")
    if feedback.comment:
        typer.echo(f"{tr('Comment')}: {feedback.comment}")
    resolution = feedback.resolution
    if resolution is None:
        typer.echo(f"{tr('Status')}: {tr('Unresolved')}")
    else:
        labels = {"acknowledged": "Acknowledged", "not_applied": "Not applied", "applied": "Applied"}
        typer.echo(f"{tr('Status')}: {tr(labels[resolution.action])}")
        if resolution.applied_revision:
            typer.echo(f"{tr('Applied revision')}: {resolution.applied_revision.address}")
        if resolution.note:
            typer.echo(f"{tr('Resolution')}: {resolution.note}")
        if resolution.mapping_note:
            typer.echo(f"{tr('Revision mapping')}: {resolution.mapping_note}")


@feedback_cli.command("record")
def record(
    ctx: typer.Context,
    ref: Annotated[str | None, typer.Argument()] = None,
    category: Category = "accuracy",
    value: Value | None = None,
    comment: str | None = None,
    author: str | None = None,
    namespace: str | None = None,
    selection: Annotated[Path | None, typer.Option("--selection", exists=True, dir_okay=False)] = None,
    query_result: Annotated[Path | None, typer.Option("--query-result", exists=True, dir_okay=False)] = None,
    candidate: str | None = None,
) -> None:
    if (ref is None) == (query_result is None):
        raise NousError("input.invalid_feedback", "Choose an exact record revision or a query result")
    if candidate is not None and query_result is None:
        raise NousError("input.invalid_feedback", "A candidate requires a query result")
    if selection is not None and query_result is not None:
        raise NousError("input.invalid_feedback", "Choose the delivered candidate from the query result")

    async def run() -> FeedbackView:
        async with open_instance(options(ctx).locator) as runtime:
            cognition = await runtime.cognition(namespace)
            if query_result is not None:
                result = QueryResult.model_validate_json(query_result.read_bytes())
                target = None
                if candidate is not None:
                    matches = [item for item in result.delivery if item.candidate_ref.key == candidate]
                    if len(matches) != 1:
                        raise NousError("input.invalid_feedback", "Candidate is not a unique delivered selection")
                    target = matches[0]
                return await cognition.feedback.record_query(
                    result, category, target=target, value=value, comment=comment, author=author
                )
            assert ref is not None
            return await cognition.feedback.record(
                ref, category, value=value, comment=comment, author=author,
                selection=ContentSelection.model_validate_json(selection.read_bytes()) if selection else None,
            )

    _show(ctx, run_async(run()))


@feedback_cli.command("read")
def read(ctx: typer.Context, ref: str, namespace: str | None = None) -> None:
    async def run() -> FeedbackView:
        async with open_instance(options(ctx).locator) as runtime:
            cognition = await runtime.cognition(namespace)
            return await cognition.feedback.read(ref)

    _show(ctx, run_async(run()))


@feedback_cli.command("list")
def list_feedback(
    ctx: typer.Context,
    target: str | None = None,
    category: Category | None = None,
    unresolved: bool = False,
    limit: Annotated[int, typer.Option(min=1, max=200)] = 50,
    offset: Annotated[int, typer.Option(min=0)] = 0,
    namespace: str | None = None,
) -> None:
    async def run() -> tuple[FeedbackView, ...]:
        async with open_instance(options(ctx).locator) as runtime:
            cognition = await runtime.cognition(namespace)
            return await cognition.feedback.list(
                target=target, category=category, unresolved_only=unresolved, limit=limit, offset=offset
            )

    result = run_async(run())
    if options(ctx).json_mode:
        emit(ctx, {"items": [item.model_dump(mode="json") for item in result]})
    elif not result:
        typer.echo(translate("No feedback", language_for(options(ctx))))
    else:
        for item in result:
            _show(ctx, item)
            typer.echo()


@feedback_cli.command("resolve")
def resolve(
    ctx: typer.Context,
    ref: str,
    action: Annotated[Literal["acknowledged", "not_applied"], typer.Option()],
    note: str | None = None,
    actor: str | None = None,
    namespace: str | None = None,
) -> None:
    async def run() -> FeedbackView:
        async with open_instance(options(ctx).locator) as runtime:
            cognition = await runtime.cognition(namespace)
            return await cognition.feedback.resolve(ref, action, note=note, actor=actor)

    _show(ctx, run_async(run()))
