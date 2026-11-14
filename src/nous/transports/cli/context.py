"""CLI call selection, terminal output, and request context."""

import asyncio
import inspect
import json
from collections.abc import Awaitable, Callable, Mapping
from contextvars import ContextVar
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, cast

import typer
from pydantic import JsonValue

from nous.application.contracts import EventEnvelope, EventObserver, OperationDefinition, Saved
from nous.application.inputs import InputProvider
from nous.common.errors import NousError
from nous.common.operations import add_error_context, exception_context, reference_value
from nous.configuration import Configuration
from nous.content.contracts import ContentReadView
from nous.presentation import format_status, select_language, translate
from nous.presentation.json_selection import select_json
from nous.runtime.instance import InstanceLocator

if TYPE_CHECKING:
    from nous.application.dispatch import Application
    from nous.runtime.execution import ExecutionManager


@dataclass(frozen=True)
class Options:
    locator: InstanceLocator
    json_mode: bool = False
    language: str | None = None
    debug: bool = False
    select: tuple[str, ...] = ()
    completed_refs: list[dict[str, JsonValue]] = field(default_factory=list, compare=False)


current_options: ContextVar[Options | None] = ContextVar("local_cli_options", default=None)


def options(ctx: typer.Context) -> Options:
    return cast(Options, ctx.obj)


def run_async[T](action: Awaitable[T]) -> T:
    """Translate cancellation after owners have released their actual workers and resources."""
    try:
        return asyncio.run(action)
    except (asyncio.CancelledError, KeyboardInterrupt, TimeoutError) as error:
        details = exception_context(error)
        cancelled = isinstance(error, (asyncio.CancelledError, KeyboardInterrupt))
        raise NousError(
            "operation.cancelled" if cancelled else "operation.timeout",
            "Operation cancelled" if cancelled else "Operation exceeded its deadline.",
            details=details,
            operation_id=cast(str | None, details.get("operation_id")),
            trace_id=cast(str | None, details.get("trace_id")),
            cause=error,
        ) from error


def application_call[Q, R](
    ctx: typer.Context,
    operation: OperationDefinition[Q, R],
    request: Q,
    *,
    inputs: Mapping[str, InputProvider] | None = None,
    observer: EventObserver | None = None,
    timeout: float | None = None,
    prepare: Callable[
        [Q, Mapping[str, InputProvider], int],
        Awaitable[tuple[Q, Mapping[str, InputProvider]]],
    ]
    | None = None,
) -> R:
    """Call the same typed operation through the host or a locally owned instance."""
    from nous.runtime.bootstrap import open_instance

    from .local_control import request_call

    selected = options(ctx)
    downstream = observer
    if selected.select:

        async def capture(event: EventEnvelope) -> None:
            if isinstance(event.event, Saved):
                for ref in event.event.refs:
                    value = reference_value(ref)
                    if value not in selected.completed_refs:
                        selected.completed_refs.append(value)
            if downstream is not None:
                outcome = downstream(event)
                if inspect.isawaitable(outcome):
                    await outcome

        observer = capture

    response = request_call(
        options(ctx).locator,
        operation.name,
        request,
        inputs=inputs,
        observer=observer,
        timeout=timeout,
        prepare=cast(
            Callable[
                [object, Mapping[str, InputProvider], int],
                Awaitable[tuple[object, Mapping[str, InputProvider]]],
            ]
            | None,
            prepare,
        ),
    )
    if response is not None:
        return operation.result_adapter.validate_python(response.get("value"))

    async def local() -> R:
        from nous.application.config_operations import (
            offline_access,
            open_configuration_application,
        )

        async def invoke(app: Application, execution: ExecutionManager) -> R:
            if prepare is not None:
                async with execution.request(operation.name, timeout=timeout) as context:
                    prepared, bindings = await prepare(
                        request, inputs or {}, context.settings.values.content.text_read_max_bytes
                    )
                    return await app.call(operation, prepared, inputs=bindings, observer=observer)
            return await app.call(
                operation, request, inputs=inputs, observer=observer, timeout=timeout
            )

        writable = offline_access(operation.name)
        if writable is not None:
            async with open_configuration_application(options(ctx).locator, writable=writable) as (
                app,
                execution,
            ):
                return await invoke(app, execution)
        async with open_instance(options(ctx).locator) as runtime:
            return await invoke(runtime.app, runtime.execution)

    return run_async(local())


def application_catalog(ctx: typer.Context, name: str | None = None) -> dict[str, object]:
    """Read the same registered definitions from the host or a local instance."""
    from nous.runtime.bootstrap import open_instance

    from .local_control import request_catalog

    response = request_catalog(options(ctx).locator, name)
    if response is not None:
        return response

    async def local() -> dict[str, object]:
        async with open_instance(options(ctx).locator) as runtime:
            if name is None:
                return {"operations": list(runtime.app.list())}
            return dict(runtime.app.describe(name))

    return run_async(local())


def language_for(selected: Options | None) -> str:
    configured: str | None = None
    if selected is not None and selected.language is None:
        try:
            configured = Configuration(
                selected.locator.config_file
            ).snapshot.values.localization.language
        except NousError, OSError:
            pass
    return select_language(selected.language if selected else None, configured)


def emit(ctx: typer.Context, value: Mapping[str, object]) -> None:
    selected = options(ctx)
    if selected.json_mode:
        try:
            output = select_json(value, selected.select)
        except NousError as error:
            if selected.completed_refs:
                add_error_context(error, {"completed_refs": selected.completed_refs})
            raise
        typer.echo(json.dumps(output, ensure_ascii=False, separators=(",", ":")))
    else:
        typer.echo(format_status(value, language_for(selected)))


def emit_content_view(view: ContentReadView) -> None:
    """Render the shared content view for text and media using existing representations."""
    language = language_for(current_options.get())
    typer.echo(f"{view.name} · {view.media_type} · {view.export_ref.identifier}")
    if view.detected_media_type and view.detected_media_type != view.media_type:
        typer.echo(f"{translate('Detected type', language)}: {view.detected_media_type}")
    selection = view.source.selection.model_dump(
        mode="json", exclude_none=True, exclude_defaults=True
    )
    if selection:
        typer.echo(
            f"{translate('Selection', language)}: {json.dumps(selection, ensure_ascii=False)}"
        )
    if view.body_text is not None:
        typer.echo(view.body_text)
    for representation in view.available_representations:
        typer.echo(
            f"  {translate(representation.kind, language)} · {representation.operation} · {representation.result_ref.identifier} · {representation.content.artifact.identifier}"
        )
        for source in representation.source_selections:
            coordinates = source.selection.model_dump(
                mode="json", exclude_none=True, exclude_defaults=True
            )
            suffix = " " + json.dumps(coordinates, ensure_ascii=False) if coordinates else ""
            typer.echo(f"  {translate('Source', language)}: {source.artifact.identifier}{suffix}")
        if representation.body_text is not None:
            typer.echo(representation.body_text)
