"""Record and resolve feedback through the cognitive application owner."""

from pathlib import Path
from typing import Annotated, Literal

import typer

from nous.cognition.feedback import (
    LIST_FEEDBACK,
    READ_FEEDBACK,
    RECORD_FEEDBACK,
    RECORD_QUERY_FEEDBACK,
    RESOLVE_FEEDBACK,
    Category,
    FeedbackView,
    ListFeedback,
    QueryFeedbackRequest,
    ReadFeedback,
    RecordFeedbackRequest,
    ResolveFeedback,
    Value,
)
from nous.common.errors import NousError
from nous.presentation import translate

from .context import application_call, emit, language_for, options
from .groups import cli
from .input_files import json_binding

feedback_cli = typer.Typer(help="Feedback", no_args_is_help=True)
cli.add_typer(feedback_cli, name="feedback")


def _show(ctx: typer.Context, feedback: FeedbackView) -> None:
    if options(ctx).json_mode:
        emit(ctx, feedback.model_dump(mode="json"))
        return
    language = language_for(options(ctx))

    def tr(message: str) -> str:
        return translate(message, language)

    typer.echo(f"{tr('Feedback')}: {feedback.identifier}")
    if feedback.target.revision is not None:
        typer.echo(f"{tr('Target')}: {feedback.target.revision.identifier}")
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
        labels = {
            "acknowledged": "Acknowledged",
            "not_applied": "Not applied",
            "applied": "Applied",
        }
        typer.echo(f"{tr('Status')}: {tr(labels[resolution.action])}")
        if resolution.applied_revision:
            typer.echo(f"{tr('Applied revision')}: {resolution.applied_revision.identifier}")
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
    selection: Annotated[
        Path | None, typer.Option("--selection", exists=True, dir_okay=False)
    ] = None,
    query_result: Annotated[
        Path | None, typer.Option("--query-result", exists=True, dir_okay=False)
    ] = None,
    passage: Annotated[
        str | None, typer.Option("--passage", help="Group:passage, starting at 1")
    ] = None,
) -> None:
    if (ref is None) == (query_result is None):
        raise NousError(
            "input.invalid_feedback", "Choose an exact record revision or a query result"
        )
    if passage is not None and query_result is None:
        raise NousError("input.invalid_feedback", "A passage requires a query result")
    if selection is not None and query_result is not None:
        raise NousError(
            "input.invalid_feedback", "Choose the delivered candidate from the query result"
        )

    if query_result is not None:
        source, inputs = json_binding(query_result, key="result")
        target = None
        if passage is not None:
            parts = passage.split(":")
            if len(parts) != 2 or not all(part.isdecimal() and int(part) > 0 for part in parts):
                raise NousError("input.invalid_feedback", "Use a 1-based group:passage pair")
            target = (int(parts[0]) - 1, int(parts[1]) - 1)
        result = application_call(
            ctx,
            RECORD_QUERY_FEEDBACK,
            QueryFeedbackRequest(
                result=source,
                category=category,
                namespace=namespace,
                target=target,
                value=value,
                comment=comment,
                author=author,
            ),
            inputs=inputs,
        )
    else:
        assert ref is not None
        selected, inputs = json_binding(selection, key="selection") if selection else (None, {})
        result = application_call(
            ctx,
            RECORD_FEEDBACK,
            RecordFeedbackRequest(
                target=ref,
                category=category,
                namespace=namespace,
                value=value,
                comment=comment,
                author=author,
                selection=selected,
            ),
            inputs=inputs,
        )
    _show(ctx, result)


@feedback_cli.command("read")
def read(ctx: typer.Context, ref: str, namespace: str | None = None) -> None:
    _show(ctx, application_call(ctx, READ_FEEDBACK, ReadFeedback(ref=ref, namespace=namespace)))


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
    result = application_call(
        ctx,
        LIST_FEEDBACK,
        ListFeedback(
            namespace=namespace,
            target=target,
            category=category,
            unresolved_only=unresolved,
            limit=limit,
            offset=offset,
        ),
    )
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
    result = application_call(
        ctx,
        RESOLVE_FEEDBACK,
        ResolveFeedback(ref=ref, action=action, namespace=namespace, note=note, actor=actor),
    )
    _show(ctx, result)
