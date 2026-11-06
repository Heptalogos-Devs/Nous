"""DBOS-backed transaction admission and job inspection/control."""

from __future__ import annotations

import asyncio
import functools
from collections.abc import AsyncGenerator, Callable, Coroutine
from contextlib import asynccontextmanager
from datetime import UTC, datetime
from typing import Concatenate, Literal, cast
from uuid import NAMESPACE_URL, uuid5

from dbos import (
    DBOSClient,
    EnqueueOptions,
    PortableWorkflowError,
    SetWorkflowID,
    WorkflowHandle,
    WorkflowSerializationFormat,
    WorkflowStatus,
)
from opentelemetry.propagate import inject
from pydantic import JsonValue
from sqlalchemy import Connection
from sqlalchemy.exc import IntegrityError

from nous.common.errors import NousError, failure_code, failure_message
from nous.common.operations import OperationIssue
from nous.runtime.execution import complete_cleanup
from nous.storage.database import WriteTx

from .configuration import JobsConfig
from .contracts import JobOutcome, JobRef, JobView
from .maintenance import CLASS_NAME, CONTRACT_VERSION, WORKFLOW_NAMES

Kind = Literal["annotation", "refresh"]
QUEUES: dict[Kind, str] = {"annotation": "nous-annotation", "refresh": "nous-refresh"}


def application_name(instance_id: str) -> str:
    return f"nous-{instance_id}"


def _timestamp(value: int | None) -> datetime | None:
    return datetime.fromtimestamp(value / 1000, UTC) if value is not None else None


def _controlled[**P, T](
    action: Callable[Concatenate[JobOperations, P], Coroutine[object, object, T]],
) -> Callable[Concatenate[JobOperations, P], Coroutine[object, object, T]]:
    @functools.wraps(action)
    async def run(self: JobOperations, *args: P.args, **kwargs: P.kwargs) -> T:
        async with self._control():  # pyright: ignore[reportPrivateUsage]
            return await action(self, *args, **kwargs)

    return run


class JobOperations:
    """Manage only this instance's explicitly registered maintenance jobs."""

    def __init__(
        self,
        client: DBOSClient,
        instance_id: str,
        settings: JobsConfig,
        current_settings: Callable[[], JobsConfig] | None = None,
    ) -> None:
        self._client = client
        self.instance_id = instance_id
        self.settings = settings
        self.current_settings = current_settings or (lambda: settings)
        self.application = application_name(instance_id)
        self._accepting = True
        self._controls: dict[asyncio.Task[object], tuple[int, asyncio.Future[None]]] = {}

    @asynccontextmanager
    async def _control(self) -> AsyncGenerator[None]:
        task = asyncio.current_task()
        if task is None:
            raise RuntimeError("Job control requires an asyncio task")
        previous = self._controls.get(task)
        if previous is None:
            if not self._accepting:
                raise NousError("job.unavailable", "The job client is stopping")
            completed = asyncio.get_running_loop().create_future()
            self._controls[task] = (1, completed)
        else:
            completed = previous[1]
            self._controls[task] = (previous[0] + 1, completed)
        try:
            yield
        finally:
            count, _ = self._controls[task]
            if count == 1:
                del self._controls[task]
                completed.set_result(None)
            else:
                self._controls[task] = (count - 1, completed)

    async def aclose(self) -> None:
        """Stop control requests and wait for their in-progress client calls to exit."""
        self._accepting = False
        current = asyncio.current_task()
        pending = [
            (task, done) for task, (_, done) in self._controls.items() if task is not current
        ]
        for task, _ in pending:
            task.cancel()
        if pending:
            await asyncio.gather(*(asyncio.shield(done) for _, done in pending))

    @_controlled
    async def enqueue_in(
        self,
        tx: WriteTx,
        *,
        kind: Kind,
        workflow_id: str,
        payload: dict[str, JsonValue],
    ) -> JobRef:
        """Register work on the caller's connection and leave its commit to the owner."""
        namespace = payload.get("namespace_ref")
        if not isinstance(namespace, dict) or namespace.get("instance_id") != self.instance_id:
            raise NousError("job.instance_mismatch", "Job namespace belongs to another instance")
        payload = dict(payload)
        if "trace" not in payload:
            carrier: dict[str, str] = {}
            inject(carrier)
            if carrier:
                payload["trace"] = {key: cast(JsonValue, value) for key, value in carrier.items()}
        options: EnqueueOptions = {
            "workflow_name": WORKFLOW_NAMES[kind],
            "class_name": CLASS_NAME,
            "instance_name": self.instance_id,
            "queue_name": QUEUES[kind],
            "workflow_id": workflow_id,
            "workflow_id_reuse_policy": "reject",
            "app_version": CONTRACT_VERSION,
            "application_name": self.application,
            "serialization_type": WorkflowSerializationFormat.PORTABLE,
        }
        timeout = self.current_settings().workflow_timeout_seconds
        if timeout is not None:
            options["workflow_timeout"] = timeout

        def enqueue(connection: Connection) -> str:
            handle = cast(
                WorkflowHandle[JsonValue],
                self._client.enqueue_in_transaction(connection, options, payload),
            )
            return handle.get_workflow_id()

        registered = await tx.connection.run_sync(enqueue)
        return JobRef(instance_id=self.instance_id, workflow_id=registered)

    async def _status(self, ref: JobRef, *, load_output: bool = True) -> WorkflowStatus:
        if ref.instance_id != self.instance_id:
            raise NousError("job.instance_mismatch", "Job belongs to another instance")
        matches = await complete_cleanup(
            asyncio.to_thread(
                self._client.list_workflows,
                workflow_ids=[ref.workflow_id],
                application_name=self.application,
                load_input=True,
                load_output=load_output,
                limit=1,
            )
        )
        if not matches:
            raise NousError(
                "job.not_found", "Job was not found", details={"job_id": ref.workflow_id}
            )
        status = matches[0]
        if (
            status.class_name != CLASS_NAME
            or status.config_name != self.instance_id
            or status.name not in WORKFLOW_NAMES.values()
            or status.app_version != CONTRACT_VERSION
        ):
            raise NousError("job.not_found", "Job does not belong to this maintenance contract")
        return status

    def _view(self, status: WorkflowStatus) -> JobView:
        # DBOS's public WorkflowStatus contains portable inputs and outputs.
        inputs = status.input
        if inputs is None or not inputs["args"] or not isinstance(inputs["args"][0], dict):
            raise NousError("job.invalid_payload", "Stored job input is invalid")
        payload = cast(dict[str, JsonValue], inputs["args"][0])
        kind: Kind = "annotation" if status.name == WORKFLOW_NAMES["annotation"] else "refresh"
        error = status.error
        issue = None
        if error is not None:
            code = (
                str(error.code)
                if isinstance(error, PortableWorkflowError) and error.code is not None
                else failure_code(error)
            )
            details = (
                cast(dict[str, JsonValue], error.data)
                if isinstance(error, PortableWorkflowError) and isinstance(error.data, dict)
                else {}
            )
            issue = OperationIssue(
                stage=kind, code=code, message=failure_message(error), details=details
            )
        return JobView.model_validate(
            {
                "ref": {"instance_id": self.instance_id, "workflow_id": status.workflow_id},
                "kind": kind,
                "status": status.status,
                "queue_name": status.queue_name or QUEUES[kind],
                "namespace_ref": payload.get("namespace_ref"),
                "target_revision": payload.get("target_revision"),
                "created_at": _timestamp(status.created_at),
                "started_at": _timestamp(status.dequeued_at),
                "completed_at": _timestamp(status.completed_at),
                "result": status.output if status.status == "SUCCESS" else None,
                "error": issue,
                "forked_from": status.forked_from,
            }
        )

    @_controlled
    async def read(self, ref: JobRef) -> JobView:
        """Read a job in this instance, including the failed step ID when it failed."""
        view = self._view(await self._status(ref))
        if view.status == "ERROR":
            steps = await complete_cleanup(
                asyncio.to_thread(self._client.list_workflow_steps, ref.workflow_id)
            )
            failed = next((step for step in reversed(steps) if step["error"] is not None), None)
            if failed is not None:
                issue = view.error
                if issue is not None:
                    issue = OperationIssue(
                        stage=failed["function_name"],
                        code=issue.code,
                        message=issue.message,
                        details=issue.details,
                        retryable=issue.retryable,
                    )
                view = view.model_copy(
                    update={"failed_step": failed["function_id"], "error": issue}
                )
        return view

    @_controlled
    async def list(
        self,
        *,
        kind: Kind | None = None,
        status: str | None = None,
        limit: int = 50,
        cursor: int = 0,
    ) -> tuple[tuple[JobView, ...], int | None]:
        """Return a bounded page and an offset to continue scanning this instance's jobs."""
        if limit < 1 or limit > 500 or cursor < 0:
            raise NousError("input.invalid", "Invalid job pagination")
        rows = await complete_cleanup(
            asyncio.to_thread(
                self._client.list_workflows,
                application_name=self.application,
                app_version=CONTRACT_VERSION,
                name=WORKFLOW_NAMES[kind] if kind else list(WORKFLOW_NAMES.values()),
                status=status,
                limit=limit + 1,
                offset=cursor,
                sort_desc=True,
                load_input=True,
                load_output=False,
            )
        )
        valid = [
            row
            for row in rows
            if row.class_name == CLASS_NAME and row.config_name == self.instance_id
        ]
        return tuple(self._view(row) for row in valid[:limit]), cursor + limit if len(
            rows
        ) > limit else None

    @_controlled
    async def has_runnable(self) -> bool:
        """Check whether the current contract has queued or executing work."""
        rows = await complete_cleanup(
            asyncio.to_thread(
                self._client.list_workflows,
                application_name=self.application,
                app_version=CONTRACT_VERSION,
                name=list(WORKFLOW_NAMES.values()),
                status=["ENQUEUED", "PENDING"],
                limit=1,
                load_input=False,
                load_output=False,
            )
        )
        return bool(rows)

    @_controlled
    async def queues(self) -> dict[str, bool]:
        """Report whether the configured maintenance queues exist for this instance."""
        queues = await complete_cleanup(
            asyncio.to_thread(self._client.list_queues, application_name=self.application)
        )
        names = {queue.name for queue in queues}
        return {name: name in names for name in QUEUES.values()}

    @_controlled
    async def wait(self, ref: JobRef, *, timeout: float | None = None) -> JobOutcome:  # noqa: ASYNC109
        """Wait without a business slot; timeout or caller cancellation leaves the job running."""
        async with asyncio.timeout(timeout):
            while True:
                view = await self.read(ref)
                if view.status == "SUCCESS":
                    assert view.result is not None
                    return view.result
                if view.status in {"ERROR", "CANCELLED", "MAX_RECOVERY_ATTEMPTS_EXCEEDED"}:
                    raise NousError(
                        "job." + view.status.lower(),
                        view.error.message if view.error else f"Job ended in {view.status}",
                        details={"job_id": ref.workflow_id},
                    )
                await asyncio.sleep(min(float(self.settings.poll_interval_seconds), 1.0))

    @_controlled
    async def cancel(self, ref: JobRef) -> JobView:
        """Record a durable cancel request; running business cleanup may still be in progress."""
        await self._status(ref, load_output=False)
        await complete_cleanup(asyncio.to_thread(self._client.cancel_workflow, ref.workflow_id))
        return await self.read(ref)

    @_controlled
    async def resume(self, ref: JobRef) -> JobView:
        """Requeue a cancelled or recovery-exhausted job on its original queue."""
        old = await self._status(ref, load_output=False)
        if old.status not in {"CANCELLED", "MAX_RECOVERY_ATTEMPTS_EXCEEDED"}:
            raise NousError(
                "job.not_resumable", "Only cancelled or recovery-exhausted jobs can resume"
            )
        kind: Kind = "annotation" if old.name == WORKFLOW_NAMES["annotation"] else "refresh"
        await complete_cleanup(
            asyncio.to_thread(
                self._client.resume_workflow, ref.workflow_id, queue_name=QUEUES[kind]
            )
        )
        return await self.read(ref)

    @_controlled
    async def retry_failed(self, ref: JobRef, from_step: int) -> JobRef:
        """Return the single retry of a failed job, preserving its completed steps and queue.

        Concurrent callers receive the same successor. Retry that successor if it
        subsequently fails; its predecessor retains the original failure.
        """
        old = await self._status(ref, load_output=False)
        if old.status != "ERROR":
            raise NousError("job.not_failed", "Only failed jobs can be retried")
        kind: Kind = "annotation" if old.name == WORKFLOW_NAMES["annotation"] else "refresh"
        if old.queue_name != QUEUES[kind]:
            raise NousError("job.queue_mismatch", "The failed job has an unexpected queue")
        steps = await complete_cleanup(
            asyncio.to_thread(self._client.list_workflow_steps, ref.workflow_id)
        )
        failed = next((step for step in reversed(steps) if step["error"] is not None), None)
        if failed is None or failed["function_id"] != from_step:
            raise NousError("job.invalid_step", "The requested step is not the failed step")
        retry = JobRef(
            instance_id=self.instance_id,
            workflow_id=str(
                uuid5(NAMESPACE_URL, f"nous:retry:{self.instance_id}:{ref.workflow_id}")
            ),
        )

        def fork() -> str:
            with SetWorkflowID(retry.workflow_id):
                handle = self._client.fork_workflow(
                    ref.workflow_id,
                    from_step,
                    application_version=CONTRACT_VERSION,
                    queue_name=old.queue_name,
                )
                return handle.get_workflow_id()

        try:
            created = await complete_cleanup(asyncio.to_thread(fork))
        except IntegrityError:
            existing = await self._status(retry, load_output=False)
            if existing.forked_from != ref.workflow_id or existing.queue_name != old.queue_name:
                raise NousError(
                    "job.retry_conflict", "Retry identity belongs to another job"
                ) from None
            return retry
        if created != retry.workflow_id:
            raise NousError("job.retry_conflict", "The retry received an unexpected identity")
        return retry
