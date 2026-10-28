"""Context commands use the SDK's measured admission and reporting contracts."""

from pathlib import Path
from uuid import uuid4

import typer
from nous_wave.contracts.context import (
    ChangeResidents,
    ChangeWorkContext,
    ContextAdmissionCommand,
    ContextCommand,
    ContextFeedback,
    ContextPrepare,
    ContextReport,
    OpenContextSession,
)

from .common import invoke, read_command


def register(cli: typer.Typer) -> None:
    group = typer.Typer(help="Prepare, admit and report actual context use.", no_args_is_help=False)
    cli.add_typer(group, name="context")

    @group.command("admit")
    def admit(ctx: typer.Context, namespace_id: str, subject_id: str, command: Path) -> None:
        value = read_command(command, ContextCommand)
        invoke(ctx, lambda api: api.admit_context(namespace_id, subject_id, value))

    @group.command("get")
    def get(ctx: typer.Context, namespace_id: str, subject_id: str, context_id: str) -> None:
        invoke(ctx, lambda api: api.read_context(namespace_id, subject_id, context_id))

    @group.command("feedback")
    def feedback(ctx: typer.Context, namespace_id: str, subject_id: str, command: Path) -> None:
        value = read_command(command, ContextFeedback)
        invoke(ctx, lambda api: api.context_feedback(namespace_id, subject_id, value))

    @group.command("prepare")
    def prepare(ctx: typer.Context, namespace_id: str, subject_id: str, command: Path) -> None:
        value = read_command(command, ContextPrepare)
        invoke(ctx, lambda api: api.prepare_context(namespace_id, subject_id, value))

    @group.command("preparation")
    def preparation(
        ctx: typer.Context, namespace_id: str, subject_id: str, preparation_id: str
    ) -> None:
        invoke(ctx, lambda api: api.context_preparation(namespace_id, subject_id, preparation_id))

    @group.command("submit")
    def submit(ctx: typer.Context, namespace_id: str, subject_id: str, command: Path) -> None:
        """Submit durable preparation; use jobs wait and context preparation for its result."""
        value = read_command(command, ContextPrepare)
        invoke(ctx, lambda api: api.submit_context_preparation(namespace_id, subject_id, value))

    @group.command("cancel")
    def cancel(
        ctx: typer.Context,
        namespace_id: str,
        subject_id: str,
        preparation_id: str,
        operation_id: str | None = None,
    ) -> None:
        invoke(
            ctx,
            lambda api: api.cancel_context_preparation(
                namespace_id, subject_id, preparation_id, operation_id or str(uuid4())
            ),
        )

    @group.command("admit-managed")
    def managed_admit(
        ctx: typer.Context, namespace_id: str, subject_id: str, command: Path
    ) -> None:
        value = read_command(command, ContextAdmissionCommand)
        invoke(ctx, lambda api: api.admit_managed_context(namespace_id, subject_id, value))

    @group.command("report")
    def report(ctx: typer.Context, namespace_id: str, subject_id: str, command: Path) -> None:
        value = read_command(command, ContextReport)
        invoke(ctx, lambda api: api.report_managed_context(namespace_id, subject_id, value))

    @group.command("open-session")
    def open_session(ctx: typer.Context, namespace_id: str, subject_id: str, command: Path) -> None:
        value = read_command(command, OpenContextSession)
        invoke(ctx, lambda api: api.open_context_session(namespace_id, subject_id, value))

    @group.command("close-session")
    def close_session(
        ctx: typer.Context,
        namespace_id: str,
        subject_id: str,
        session_id: str,
        operation_id: str | None = None,
    ) -> None:
        invoke(
            ctx,
            lambda api: api.close_context_session(
                namespace_id, subject_id, session_id, operation_id or str(uuid4())
            ),
        )

    @group.command("session")
    def session(ctx: typer.Context, namespace_id: str, subject_id: str, session_id: str) -> None:
        invoke(ctx, lambda api: api.context_session(namespace_id, subject_id, session_id))

    @group.command("residents")
    def residents(ctx: typer.Context, namespace_id: str, subject_id: str, command: Path) -> None:
        value = read_command(command, ChangeResidents)
        invoke(ctx, lambda api: api.change_context_residents(namespace_id, subject_id, value))

    @group.command("set-work")
    def set_work(ctx: typer.Context, namespace_id: str, subject_id: str, command: Path) -> None:
        value = read_command(command, ChangeWorkContext)
        invoke(ctx, lambda api: api.change_work_context(namespace_id, subject_id, value))

    @group.command("work")
    def work(ctx: typer.Context, namespace_id: str, subject_id: str, work_context_id: str) -> None:
        invoke(ctx, lambda api: api.work_context(namespace_id, subject_id, work_context_id))
