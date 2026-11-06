"""Job control commands and the local durable worker host."""

import asyncio
import os
import shlex
from typing import Annotated, Literal

import typer

from nous.jobs.client import open_job_client
from nous.jobs.contracts import JobOutcome, JobRef, JobView
from nous.presentation import translate
from nous.runtime.bootstrap import open_instance

from . import emit, language_for, options, run_async, runtime_cli

job_cli = typer.Typer(help="查看和控制维护工作 / Durable maintenance jobs", no_args_is_help=True)


def _command(ctx: typer.Context, *args: str) -> str:
    locator = options(ctx).locator
    selection = (
        ["--profile", locator.profile]
        if locator.profile
        else ["--instance", str(locator.directory)]
    )
    parts = ["nous", *selection, "job", *args]
    if os.name != "nt":
        return shlex.join(parts)
    return " ".join(
        "'" + part.replace("'", "''") + "'"
        if index == 2 or not part or any(not (char.isalnum() or char in "._-/\\:") for char in part)
        else part
        for index, part in enumerate(parts)
    )


def _kind(kind: str, language: str) -> str:
    return translate("Annotation" if kind == "annotation" else "Retrieval refresh", language)


def _status(status: str, language: str) -> str:
    labels = {
        "ENQUEUED": "Queued",
        "DELAYED": "Scheduled",
        "PENDING": "Running",
        "SUCCESS": "Completed",
        "ERROR": "Failed",
        "CANCELLED": "Cancelled",
        "MAX_RECOVERY_ATTEMPTS_EXCEEDED": "Recovery limit reached",
    }
    return translate(labels.get(status, status), language)


def _target(job: JobView, language: str) -> str:
    if job.target_revision is not None:
        return job.target_revision.identifier
    return f"{translate('Namespace', language)} {job.namespace_ref.id}"


def _outcome(outcome: JobOutcome, language: str) -> None:
    states = {
        "ready": "Ready",
        "skipped": "Skipped",
        "superseded": "Superseded",
        "published_not_current": "Published; newer changes are pending",
    }
    typer.echo(f"{translate('Outcome', language)}: {translate(states[outcome.state], language)}")
    shown: set[str] = set()
    if outcome.proposal_result is not None:
        identifier = outcome.proposal_result.identifier
        typer.echo(f"{translate('Proposal', language)}: {identifier}")
        shown.add(identifier)
    if outcome.annotation_ref is not None:
        identifier = outcome.annotation_ref.identifier
        activation = (
            f" · {translate(outcome.activation_state, language)}"
            if outcome.activation_state
            else ""
        )
        typer.echo(f"{translate('Annotation', language)}: {identifier}{activation}")
        shown.add(identifier)
    for result in outcome.result_refs:
        if result not in shown:
            typer.echo(f"{translate('Result', language)}: {result}")
    for related in outcome.related_jobs:
        typer.echo(f"{translate('Related job', language)}: {related.workflow_id}")


def _emit_job(ctx: typer.Context, job: JobView) -> None:
    if options(ctx).json_mode:
        emit(ctx, job.model_dump(mode="json"))
        return
    language = language_for(options(ctx))
    typer.echo(f"{_kind(job.kind, language)} · {_status(job.status, language)}")
    typer.echo(f"{translate('Target', language)}: {_target(job, language)}")
    typer.echo(f"{translate('Job', language)}: {job.ref.workflow_id}")
    if job.forked_from is not None:
        typer.echo(f"{translate('Retried from', language)}: {job.forked_from}")
    if job.error is not None:
        typer.echo(
            f"{translate('Error', language)}: {job.error.code} — "
            f"{translate(job.error.message, language)}"
        )
        if job.failed_step is not None:
            typer.echo(
                f"{translate('Failed step', language)}: {job.failed_step} · {job.error.stage}"
            )
    if job.result is not None:
        _outcome(job.result, language)
    if job.status == "ERROR" and job.failed_step is not None:
        typer.echo(
            f"{translate('Retry', language)}: "
            + _command(ctx, "retry", job.ref.workflow_id, "--from-step", str(job.failed_step))
        )
    elif job.status in {"CANCELLED", "MAX_RECOVERY_ATTEMPTS_EXCEEDED"}:
        typer.echo(
            f"{translate('Resume', language)}: {_command(ctx, 'resume', job.ref.workflow_id)}"
        )
    elif job.status in {"ENQUEUED", "DELAYED", "PENDING"}:
        typer.echo(f"{translate('Wait', language)}: {_command(ctx, 'wait', job.ref.workflow_id)}")


@job_cli.command("list")
def list_jobs(
    ctx: typer.Context,
    kind: Annotated[Literal["annotation", "refresh"] | None, typer.Option()] = None,
    status: Annotated[str | None, typer.Option()] = None,
    limit: Annotated[int, typer.Option(min=1, max=500)] = 50,
    cursor: Annotated[int, typer.Option(min=0)] = 0,
) -> None:
    async def action() -> None:
        async with open_job_client(options(ctx).locator) as jobs:
            values, continuation = await jobs.list(
                kind=kind, status=status, limit=limit, cursor=cursor
            )
            if options(ctx).json_mode:
                emit(
                    ctx,
                    {
                        "jobs": [value.model_dump(mode="json") for value in values],
                        "cursor": continuation,
                    },
                )
                return
            language = language_for(options(ctx))
            if not values:
                typer.echo(translate("No jobs", language))
            for value in values:
                typer.echo(
                    f"{_kind(value.kind, language)} · {_status(value.status, language)} · "
                    f"{_target(value, language)} · {value.ref.workflow_id}"
                )
            if continuation is not None:
                typer.echo(f"{translate('Next page', language)}: --cursor {continuation}")

    run_async(action())


@job_cli.command("read")
def read_job(ctx: typer.Context, job: str) -> None:
    async def action() -> None:
        async with open_job_client(options(ctx).locator) as jobs:
            value = await jobs.read(JobRef(instance_id=jobs.instance_id, workflow_id=job))
            _emit_job(ctx, value)

    run_async(action())


@job_cli.command("wait")
def wait_job(
    ctx: typer.Context, job: str, timeout: Annotated[float | None, typer.Option(min=0)] = None
) -> None:
    async def action() -> None:
        async with open_job_client(options(ctx).locator) as jobs:
            value = await jobs.wait(
                JobRef(instance_id=jobs.instance_id, workflow_id=job), timeout=timeout
            )
            if options(ctx).json_mode:
                emit(ctx, value.model_dump(mode="json"))
            else:
                typer.echo(f"{translate('Job', language_for(options(ctx)))}: {job}")
                _outcome(value, language_for(options(ctx)))

    run_async(action())


@job_cli.command("cancel")
def cancel_job(ctx: typer.Context, job: str) -> None:
    async def action() -> None:
        async with open_job_client(options(ctx).locator) as jobs:
            value = await jobs.cancel(JobRef(instance_id=jobs.instance_id, workflow_id=job))
            _emit_job(ctx, value)

    run_async(action())


@job_cli.command("resume")
def resume_job(ctx: typer.Context, job: str) -> None:
    async def action() -> None:
        async with open_job_client(options(ctx).locator) as jobs:
            value = await jobs.resume(JobRef(instance_id=jobs.instance_id, workflow_id=job))
            _emit_job(ctx, value)

    run_async(action())


@job_cli.command("retry")
def retry_job(
    ctx: typer.Context, job: str, from_step: Annotated[int, typer.Option("--from-step", min=0)]
) -> None:
    async def action() -> None:
        async with open_job_client(options(ctx).locator) as jobs:
            value = await jobs.retry_failed(
                JobRef(instance_id=jobs.instance_id, workflow_id=job), from_step
            )
            if options(ctx).json_mode:
                emit(ctx, value.model_dump(mode="json"))
            else:
                language = language_for(options(ctx))
                typer.echo(f"{translate('Retry job', language)}: {value.workflow_id}")
                typer.echo(
                    f"{translate('Wait', language)}: {_command(ctx, 'wait', value.workflow_id)}"
                )

    run_async(action())


@job_cli.command("run")
def run_jobs(
    ctx: typer.Context, until_idle: Annotated[bool, typer.Option("--until-idle")] = False
) -> None:
    """Run workers until currently runnable work has finished."""
    if not until_idle:
        raise typer.BadParameter("Use --until-idle to run the current queue")

    async def action() -> None:
        async with open_instance(options(ctx).locator, workers=True) as runtime:
            jobs = runtime.jobs
            quiet = False
            while True:
                active = await jobs.has_runnable()
                running = runtime.job_runtime.bridge.in_flight if runtime.job_runtime.bridge else 0
                if not active and not running:
                    if quiet:
                        break
                    quiet = True
                else:
                    quiet = False
                await asyncio.sleep(float(jobs.settings.poll_interval_seconds))
            delayed, _ = await jobs.list(status="DELAYED", limit=500)
            emit(ctx, {"state": "idle", "delayed": len(delayed)})

    run_async(action())


@runtime_cli.command("run")
def run_host(ctx: typer.Context) -> None:
    """Run the instance worker host until interrupted."""

    async def action() -> None:
        async with open_instance(options(ctx).locator, workers=True):
            try:
                await asyncio.Event().wait()
            except asyncio.CancelledError:
                pass

    run_async(action())
