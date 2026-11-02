"""DBOS owns durable workflow execution; Nous owns job scope and business facts."""

from __future__ import annotations

import asyncio
import inspect
import hashlib
import json
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from datetime import UTC, datetime
from functools import wraps
from typing import ParamSpec, TypeVar, cast
from uuid import uuid4

import sqlalchemy as sa
from dbos import (
    DBOS,
    DBOSClient,
    DBOSPortableJSONSerializer,
    EnqueueOptions,
    PortableWorkflowError,
    WorkflowHandle,
    WorkflowHandleAsync,
    WorkflowSerializationFormat,
    WorkflowStatus,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.execution import ExecutionFailure, ExecutionJob, ExecutionSnapshot
from opentelemetry.context import get_current
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import event
from sqlalchemy.pool import ConnectionPoolEntry

from nous.platform.errors import NousError
from nous.platform.observability import configure_tracing
from nous.platform.persistence import Authority, ReadView, WriteTx
from nous.platform.persistence.authority import SQLiteConnection, canonical_json
from nous.platform.persistence.schema import job_links

from .batches import process_jobs, retry_jobs, summarize_jobs
from .query import JobPage, query_jobs
from .results import TERMINAL, job_results, retained_status
from .scope import JobPurpose, require_job_in

P = ParamSpec("P")
R = TypeVar("R")


@dataclass(frozen=True)
class JobSpec:
    workflow_name: str
    queue_name: str
    args: tuple[JsonValue, ...] = ()
    kwargs: dict[str, JsonValue] = field(default_factory=dict)
    workflow_id: str | None = None
    owned_parent: str | None = None
    partition_key: str | None = None
    delay_seconds: float | None = None
    scope: str = ""


@dataclass(frozen=True)
class JobRef:
    workflow_id: str


@dataclass(frozen=True)
class QueueSpec:
    name: str
    worker_concurrency: int = 4
    global_concurrency: int | None = None
    partition_concurrency: int | None = None
    polling_interval_sec: float = 0.1


class DurableExecution:
    retry_jobs = retry_jobs
    summarize_jobs = summarize_jobs
    process_jobs = process_jobs

    def __init__(
        self,
        authority: Authority,
        *,
        application_name: str = "nous-wave",
        queues: tuple[QueueSpec, ...] = (QueueSpec("default"),),
    ):
        self.authority = authority
        self.application_name = application_name
        self.queues = queues
        self._workflows: set[str] = set()
        self._client: DBOSClient | None = None
        self._engine: sa.Engine | None = None
        self._started = False
        self._host_loop: asyncio.AbstractEventLoop | None = None

    @property
    def client(self) -> DBOSClient:
        if self._client is None:
            raise RuntimeError("Durable execution has not started")
        return self._client

    def register(
        self, name: str, function: Callable[P, R], *, preemptible: bool = False
    ) -> Callable[P, R]:
        if self._started:
            raise RuntimeError("Register workflows before starting execution")
        if name in self._workflows:
            raise ValueError(f"Workflow {name!r} is already registered")
        # DBOS decorates the function object with replay metadata. A bound method
        # is immutable, so own an ordinary function while retaining its signature.
        if inspect.iscoroutinefunction(function):
            async_function = cast(Callable[P, Awaitable[object]], function)

            @wraps(function)
            async def execute_async(*args: P.args, **kwargs: P.kwargs) -> object:
                if preemptible:
                    return await self._call_host(async_function, *args, **kwargs)
                return await async_function(*args, **kwargs)

            execute = (
                DBOS.step(name=f"{name}.execute", preemptible=True, retries_allowed=False)(
                    execute_async
                )
                if preemptible
                else execute_async
            )

            @wraps(function)
            async def invoke_async(*args: P.args, **kwargs: P.kwargs) -> object:
                return await execute(*args, **kwargs)

            wrapped = cast(
                Callable[P, R],
                DBOS.workflow(name=name, serialization_type=WorkflowSerializationFormat.PORTABLE)(
                    invoke_async
                ),
            )
        else:
            if preemptible:
                raise ValueError("Preemptible execution requires an async callable")

            @wraps(function)
            def invoke(*args: P.args, **kwargs: P.kwargs) -> R:
                return function(*args, **kwargs)

            wrapped = DBOS.workflow(
                name=name, serialization_type=WorkflowSerializationFormat.PORTABLE
            )(invoke)
        self._workflows.add(name)
        return wrapped

    async def start(self) -> None:
        if self._started:
            return
        if not self.authority.initialized:
            raise RuntimeError("Initialize the authority schema before starting workers")
        configure_tracing()
        self._host_loop = asyncio.get_running_loop()
        self._engine = sa.create_engine(
            sa.URL.create("sqlite", database=str(self.authority.path)),
            connect_args={"check_same_thread": False},
        )

        @event.listens_for(self._engine, "connect")
        def connect(dbapi_connection: SQLiteConnection, _record: ConnectionPoolEntry) -> None:
            cursor = dbapi_connection.cursor()
            cursor.execute("PRAGMA foreign_keys=ON")
            cursor.execute("PRAGMA busy_timeout=5000")
            cursor.close()

        serializer = DBOSPortableJSONSerializer()
        try:
            DBOS(
                config={
                    "name": self.application_name,
                    "system_database_engine": self._engine,
                    "serializer": serializer,
                    # Use the process provider without DBOS-owned network exporters.
                    "enable_otlp": True,
                    "otlp_traces_endpoints": [],
                    "otlp_logs_endpoints": [],
                    "use_listen_notify": False,
                    "executor_id": "nous-local-host",
                }
            )
            # launch runs DBOS's own formal migrations and recovery before polling.
            await asyncio.to_thread(DBOS.launch)
            self._client = DBOSClient(
                system_database_engine=self._engine,
                serializer=serializer,
                application_name=self.application_name,
            )
            for queue in self.queues:
                # The DBOS class async API replaces the caller loop's default
                # executor. Keep its owned executor on its own workflow loop.
                await asyncio.to_thread(
                    DBOS.register_queue,
                    queue.name,
                    worker_concurrency=queue.worker_concurrency,
                    global_concurrency=queue.global_concurrency,
                    partition_concurrency=queue.partition_concurrency,
                    polling_interval_sec=queue.polling_interval_sec,
                    on_conflict="always_update",
                )
            self.authority.execution = self
            self._started = True
        except BaseException:
            await self.close()
            raise

    async def _call_host[**Params](
        self,
        function: Callable[Params, Awaitable[object]],
        *args: Params.args,
        **kwargs: Params.kwargs,
    ) -> object:
        host = self._host_loop
        if host is None or host.is_closed():
            raise RuntimeError("The application event loop is unavailable")
        if asyncio.get_running_loop() is host:
            return await function(*args, **kwargs)
        owned: asyncio.Task[object] | None = None

        async def invoke() -> object:
            nonlocal owned
            owned = asyncio.current_task()
            return await function(*args, **kwargs)

        pending = asyncio.run_coroutine_threadsafe(invoke(), host)
        try:
            return await asyncio.wrap_future(pending)
        except asyncio.CancelledError as cancelled:
            pending.cancel()

            async def join() -> None:
                # Scheduled after cancellation has reached the host loop. A task
                # cancelled before invoke starts owns no application resources.
                if owned is not None:
                    await asyncio.gather(owned, return_exceptions=True)

            cleanup = asyncio.wrap_future(asyncio.run_coroutine_threadsafe(join(), host))
            while not cleanup.done():
                try:
                    await asyncio.shield(cleanup)
                except asyncio.CancelledError:
                    continue
            cleanup.result()
            raise cancelled

    async def enqueue_in(self, tx: WriteTx, spec: JobSpec) -> JobRef:
        if tx.authority is not self.authority:
            raise ValueError("Job and transaction must use the same authority database")
        if spec.workflow_name not in self._workflows:
            raise ValueError(f"Unknown workflow {spec.workflow_name!r}")
        if spec.queue_name not in {queue.name for queue in self.queues}:
            raise ValueError(f"Unknown queue {spec.queue_name!r}")
        # Only portable JSON values enter durable execution, never model objects.
        arguments = json.loads(canonical_json({"args": spec.args, "kwargs": spec.kwargs}))
        workflow_id = spec.workflow_id or str(uuid4())
        command_digest = hashlib.sha256(
            canonical_json(
                {
                    "workflow": spec.workflow_name,
                    "queue": spec.queue_name,
                    "arguments": arguments,
                    "scope": spec.scope,
                    "owned_parent": spec.owned_parent,
                    "partition_key": spec.partition_key,
                    "delay_seconds": spec.delay_seconds,
                }
            ).encode()
        ).hexdigest()
        existing = (
            (
                await tx.connection.execute(
                    sa.select(job_links).where(job_links.c.workflow_id == workflow_id)
                )
            )
            .mappings()
            .first()
        )
        if existing is not None:
            if (existing["caller_id"], existing["namespace_id"], existing["command_digest"]) != (
                tx.ctx.caller_id,
                tx.ctx.namespace_id,
                command_digest,
            ):
                raise DomainError(
                    "job_identity_conflict", "Durable work identity has different scope or command"
                )
            return JobRef(workflow_id)
        if spec.owned_parent is not None:
            if spec.owned_parent == workflow_id:
                raise ValueError("A job cannot own itself")
            parent = await tx.connection.scalar(
                sa.select(job_links.c.workflow_id).where(
                    job_links.c.workflow_id == spec.owned_parent,
                    job_links.c.caller_id == tx.ctx.caller_id,
                    job_links.c.namespace_id == tx.ctx.namespace_id,
                )
            )
            if parent is None:
                raise LookupError("Owned parent is unavailable in this caller's namespace")
            if spec.owned_parent not in tx.jobs:
                parent_handle: WorkflowHandleAsync[
                    JsonValue
                ] = await self.client.retrieve_workflow_async(spec.owned_parent)
                parent_status = await parent_handle.get_status()
                if parent_status.status not in ("PENDING", "ENQUEUED", "DELAYED"):
                    raise ValueError("A completed or cancelled job cannot admit new owned work")
        options: EnqueueOptions = {
            "workflow_name": spec.workflow_name,
            "queue_name": spec.queue_name,
            "workflow_id": workflow_id,
            "serialization_type": WorkflowSerializationFormat.PORTABLE,
            "workflow_id_reuse_policy": "reject",
            "authenticated_user": tx.ctx.caller_id,
            # DBOS persists W3C trace context only; baggage and payload are excluded.
            "otel_context": get_current(),
            "attributes": {
                "scope": spec.scope,
                "request_id": tx.ctx.request_id,
                "namespace_id": tx.ctx.namespace_id,
            },
        }
        if spec.partition_key is not None:
            options["queue_partition_key"] = spec.partition_key
        if spec.delay_seconds is not None:
            options["delay_seconds"] = spec.delay_seconds

        def enqueue(connection: sa.Connection) -> WorkflowHandle[JsonValue]:
            return self.client.enqueue_in_transaction(
                connection, options, *arguments["args"], **arguments["kwargs"]
            )

        handle = await tx.connection.run_sync(enqueue)
        await tx.connection.execute(
            job_links.insert().values(
                workflow_id=handle.get_workflow_id(),
                command_digest=command_digest,
                commit_seq=tx.commit_seq,
                caller_id=tx.ctx.caller_id,
                namespace_id=tx.ctx.namespace_id,
                scope=spec.scope,
                owned_parent=spec.owned_parent,
                workflow_name=spec.workflow_name,
                queue_name=spec.queue_name,
            )
        )
        return JobRef(handle.get_workflow_id())

    async def export_namespace_in(
        self, view: ReadView, ctx: CallContext, *, workflow_ids: tuple[str, ...] | None = None
    ) -> ExecutionSnapshot:
        from .scope import require_job_purpose

        require_job_purpose(ctx, "management")
        query = sa.select(job_links).where(job_links.c.namespace_id == ctx.namespace_id)
        if workflow_ids is not None:
            query = query.where(job_links.c.workflow_id.in_(workflow_ids))
        links = (await view.connection.execute(query)).mappings().all()
        jobs: list[ExecutionJob] = []
        for offset in range(0, len(links), 200):
            batch = links[offset : offset + 200]
            identities = [str(row["workflow_id"]) for row in batch]
            retained = {
                str(row.workflow_id): ExecutionJob.model_validate(row.record)
                for row in (
                    await view.connection.execute(
                        sa.select(job_results.c.workflow_id, job_results.c.record).where(
                            job_results.c.workflow_id.in_(identities),
                            job_results.c.namespace_id == ctx.namespace_id,
                        )
                    )
                ).all()
            }
            pending = [identity for identity in identities if identity not in retained]
            actual = (
                await self.client.list_workflows_async(
                    workflow_ids=pending, limit=len(pending), load_input=True, load_output=True
                )
                if pending
                else []
            )
            by_id = {job.workflow_id: job for job in actual}
            for row in batch:
                identity = str(row["workflow_id"])
                if identity in retained:
                    jobs.append(retained[identity])
                    continue
                status = by_id.get(identity)
                if status is None or status.input is None:
                    raise DomainError(
                        "execution_snapshot_incomplete",
                        "An admitted job has no retained command",
                        workflow_id=identity,
                    )
                failure = None
                if isinstance(status.error, PortableWorkflowError):
                    failure = ExecutionFailure(
                        name=status.error.name,
                        code=str(status.error.code or "job_failed"),
                        message=status.error.message,
                        data=TypeAdapter(JsonValue).validate_python(status.error.data),
                    )
                elif status.error is not None:
                    failure = ExecutionFailure(
                        name=type(status.error).__name__,
                        code="job_failed",
                        message=str(status.error),
                    )
                jobs.append(
                    ExecutionJob(
                        workflow_id=identity,
                        workflow_name=str(row["workflow_name"]),
                        queue_name=str(row["queue_name"]),
                        caller_id=str(row["caller_id"]),
                        commit_seq=int(row["commit_seq"]),
                        scope=str(row["scope"]),
                        owned_parent=str(row["owned_parent"]) if row["owned_parent"] else None,
                        args=TypeAdapter(tuple[JsonValue, ...]).validate_python(
                            status.input["args"]
                        ),
                        kwargs=TypeAdapter(dict[str, JsonValue]).validate_python(
                            status.input["kwargs"]
                        ),
                        state=status.status,
                        result=TypeAdapter(JsonValue).validate_python(status.output),
                        error=failure,
                        partition_key=status.queue_partition_key,
                        delay_until_epoch_ms=status.delay_until_epoch_ms,
                        created_at=status.created_at,
                        completed_at=status.completed_at,
                    )
                )
        return ExecutionSnapshot(
            namespace_id=ctx.namespace_id, authority_commit_seq=view.commit_seq, jobs=tuple(jobs)
        )

    async def restore_namespace_in(
        self, tx: WriteTx, ctx: CallContext, snapshot: ExecutionSnapshot
    ) -> tuple[str, ...]:
        from .scope import require_job_purpose

        require_job_purpose(ctx, "management")
        if snapshot.namespace_id != ctx.namespace_id:
            raise DomainError(
                "execution_snapshot_scope", "Job snapshot must name its receiving Namespace"
            )
        identities = {job.workflow_id for job in snapshot.jobs}
        if len(identities) != len(snapshot.jobs):
            raise DomainError("execution_snapshot_identity", "Task identities must be unique")
        restored: list[str] = []
        for job in snapshot.jobs:
            if job.state not in TERMINAL and (
                job.workflow_name not in self._workflows
                or job.queue_name not in {queue.name for queue in self.queues}
            ):
                raise DomainError(
                    "execution_workflow_unavailable",
                    "A restored task requires an unavailable workflow or queue",
                    workflow_name=job.workflow_name,
                    queue_name=job.queue_name,
                )
            if job.owned_parent is not None and job.owned_parent not in identities:
                parent = await tx.connection.scalar(
                    sa.select(job_links.c.workflow_id).where(
                        job_links.c.workflow_id == job.owned_parent,
                        job_links.c.namespace_id == ctx.namespace_id,
                    )
                )
                if parent is None:
                    raise DomainError(
                        "execution_snapshot_parent", "A restored task has an unavailable owner"
                    )
            existing = await tx.connection.scalar(
                sa.select(job_links.c.workflow_id).where(job_links.c.workflow_id == job.workflow_id)
            )
            if existing is not None:
                raise DomainError(
                    "execution_identity_conflict",
                    "A restored task identity already exists",
                    workflow_id=job.workflow_id,
                )
            if job.state in TERMINAL:
                await tx.connection.execute(
                    job_links.insert().values(
                        workflow_id=job.workflow_id,
                        command_digest=hashlib.sha256(
                            canonical_json(
                                {
                                    "workflow": job.workflow_name,
                                    "args": job.args,
                                    "kwargs": job.kwargs,
                                }
                            ).encode()
                        ).hexdigest(),
                        commit_seq=tx.commit_seq,
                        caller_id=job.caller_id,
                        namespace_id=ctx.namespace_id,
                        scope=job.scope,
                        owned_parent=job.owned_parent,
                        workflow_name=job.workflow_name,
                        queue_name=job.queue_name,
                    )
                )
                await tx.connection.execute(
                    job_results.insert().values(
                        workflow_id=job.workflow_id,
                        namespace_id=ctx.namespace_id,
                        record=job.model_dump(mode="json"),
                    )
                )
                continue
            delay = (
                max(0.0, (job.delay_until_epoch_ms / 1000) - datetime.now(UTC).timestamp())
                if job.delay_until_epoch_ms is not None
                else None
            )
            # Re-admit the saved domain command through DBOS's transaction API.
            # Ownership is already admitted in the snapshot; no running parent is required.
            await tx.enqueue(
                JobSpec(
                    workflow_name=job.workflow_name,
                    queue_name=job.queue_name,
                    workflow_id=job.workflow_id,
                    scope=job.scope,
                    args=job.args,
                    kwargs=job.kwargs,
                    partition_key=job.partition_key,
                    delay_seconds=delay,
                )
            )
            await tx.connection.execute(
                job_links.update()
                .where(job_links.c.workflow_id == job.workflow_id)
                .values(
                    commit_seq=tx.commit_seq,
                    caller_id=job.caller_id,
                    scope=job.scope,
                    owned_parent=job.owned_parent,
                )
            )
            restored.append(job.workflow_id)
        return tuple(restored)

    async def retained_job(self, ctx: CallContext, workflow_id: str) -> ExecutionJob | None:
        async with self.authority.read(ctx) as view:
            raw = await view.connection.scalar(
                sa.select(job_results.c.record).where(
                    job_results.c.workflow_id == workflow_id,
                    job_results.c.namespace_id == ctx.namespace_id,
                )
            )
        return ExecutionJob.model_validate(raw) if raw is not None else None

    async def _require_job(
        self, ctx: CallContext, workflow_id: str, *, purpose: JobPurpose = "actor"
    ) -> None:
        async with self.authority.read(ctx) as view:
            await require_job_in(view, ctx, workflow_id, purpose=purpose)

    async def status(
        self, ctx: CallContext, workflow_id: str, *, purpose: JobPurpose = "actor"
    ) -> WorkflowStatus:
        await self._require_job(ctx, workflow_id, purpose=purpose)
        retained = await self.retained_job(ctx, workflow_id)
        if retained is not None:
            return retained_status(retained)
        handle: WorkflowHandleAsync[JsonValue] = await self.client.retrieve_workflow_async(
            workflow_id
        )
        return await handle.get_status()

    async def list_jobs(
        self, ctx: CallContext, *, scope: str | None = None, limit: int = 100
    ) -> list[WorkflowStatus]:
        page = await self.query_jobs(ctx, scope=scope, limit=limit)
        return list(page.items)

    async def query_jobs(
        self,
        ctx: CallContext,
        *,
        scope: str | None = None,
        kind: str | None = None,
        state: str | None = None,
        limit: int = 50,
        cursor: str | None = None,
        purpose: JobPurpose = "actor",
        workflow_ids: tuple[str, ...] | None = None,
    ) -> JobPage:
        return await query_jobs(
            self.authority,
            self.client,
            ctx,
            scope=scope,
            kind=kind,
            state=state,
            limit=limit,
            cursor=cursor,
            purpose=purpose,
            workflow_ids=workflow_ids,
        )

    async def wait(
        self, ctx: CallContext, workflow_id: str, *, purpose: JobPurpose = "actor"
    ) -> JsonValue:
        await self._require_job(ctx, workflow_id, purpose=purpose)
        retained = await self.retained_job(ctx, workflow_id)
        if retained is not None:
            if retained.state == "SUCCESS":
                return retained.result
            if retained.error is not None:
                raise DomainError(
                    retained.error.code, retained.error.message, details=retained.error.data
                )
            raise DomainError(
                "job_" + retained.state.lower(),
                "Retained task ended with " + retained.state,
                workflow_id=workflow_id,
            )
        handle: WorkflowHandleAsync[JsonValue] = await self.client.retrieve_workflow_async(
            workflow_id
        )
        try:
            return await handle.get_result(polling_interval_sec=0.05)
        except PortableWorkflowError as error:
            value = TypeAdapter(JsonValue).validate_python(error.data)
            details = value if isinstance(value, dict) else {"data": value}
            code = str(error.code) if error.code is not None else "job_failed"
            if error.name == "DomainError":
                raise DomainError(code, error.message, **details) from error
            raise NousError(
                code, error.message, status=503 if code == "Unavailable" else 400, context=details
            ) from error

    async def cancel(
        self, ctx: CallContext, workflow_id: str, *, purpose: JobPurpose = "actor"
    ) -> None:
        await self._require_job(ctx, workflow_id, purpose=purpose)
        if await self.retained_job(ctx, workflow_id) is not None:
            return
        # Mark the parent first. Its SQLite write waits for any in-progress child
        # admission to commit; subsequent admissions see a terminal parent. The
        # following ownership snapshot therefore includes every accepted child.
        await self.client.cancel_workflow_async(workflow_id, cancel_children=True)
        # These are ownership edges only; shared work has no owned_parent edge.
        async with self.authority.read(ctx) as view:
            owned = list(
                (
                    await view.connection.execute(
                        sa.select(job_links.c.workflow_id).where(
                            job_links.c.owned_parent == workflow_id,
                            job_links.c.namespace_id == ctx.namespace_id,
                            sa.true()
                            if purpose == "management"
                            else job_links.c.caller_id == ctx.caller_id,
                        )
                    )
                ).scalars()
            )
        for child in owned:
            await self.cancel(ctx, child, purpose=purpose)

    async def resume(
        self, ctx: CallContext, workflow_id: str, *, purpose: JobPurpose = "actor"
    ) -> None:
        await self._require_job(ctx, workflow_id, purpose=purpose)
        retained = await self.retained_job(ctx, workflow_id)
        if retained is not None:
            if retained.state == "SUCCESS":
                raise DomainError(
                    "job_completed", "The retained task already completed", workflow_id=workflow_id
                )
            async with self.authority.write(ctx) as tx:
                await tx.connection.execute(
                    job_results.delete().where(job_results.c.workflow_id == workflow_id)
                )
                await tx.connection.execute(
                    job_links.delete().where(job_links.c.workflow_id == workflow_id)
                )
                await tx.enqueue(
                    JobSpec(
                        workflow_name=retained.workflow_name,
                        queue_name=retained.queue_name,
                        workflow_id=workflow_id,
                        args=retained.args,
                        kwargs=retained.kwargs,
                        partition_key=retained.partition_key,
                        scope=ctx.namespace_id,
                    )
                )
            return
        await self.client.resume_workflow_async(workflow_id)

    async def purge_namespace_jobs(self, ctx: CallContext, schedules: tuple[str, ...] = ()) -> None:
        from nous.identity.access import require_namespace_in

        from .scope import require_job_purpose

        require_job_purpose(ctx, "management")
        async with self.authority.read(ctx) as view:
            namespace = await require_namespace_in(view, ctx, purpose="management")
            if not namespace.purging or namespace.state != "trashed":
                raise DomainError(
                    "namespace_purge_scope_forbidden",
                    "Job removal requires the admitted complete Namespace",
                )
            identities = [
                str(value)
                for value in (
                    await view.connection.execute(
                        sa.select(job_links.c.workflow_id).where(
                            job_links.c.namespace_id == ctx.namespace_id
                        )
                    )
                ).scalars()
            ]
        for schedule in schedules:
            await self.client.delete_schedule_async(schedule)
        if identities:
            await self.client.cancel_workflows_async(identities, cancel_children=True)
            await self.client.delete_workflows_async(identities, delete_children=True)

    async def close(self) -> None:
        if self._engine is None:
            return
        self.authority.execution = None
        if self._client is not None:
            await asyncio.to_thread(self._client.destroy)
            self._client = None
        await asyncio.to_thread(
            DBOS.destroy, destroy_registry=True, workflow_completion_timeout_sec=0
        )
        await asyncio.to_thread(self._engine.dispose)
        self._engine = None
        self._started = False
        self._host_loop = None
