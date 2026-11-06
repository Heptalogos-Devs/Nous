"""Durable admission, recovery and the runtime's actual cancellation boundary."""

import asyncio
import concurrent.futures
import logging
import sqlite3
from pathlib import Path
from uuid import uuid4

import pytest
from dbos import DBOS

from nous.common.errors import NousError
from nous.configuration.definitions import (
    DatabaseConfig,
    ExecutionConfig,
    InstanceConfig,
    NousConfig,
)
from nous.configuration.service import ConfigSnapshot, FrozenValues
from nous.jobs.bridge import JsonObject, RuntimeBridge
from nous.jobs.client import open_job_client
from nous.jobs.configuration import JobsConfig
from nous.jobs.contracts import JobRef
from nous.jobs.runtime import initialize_jobs, open_jobs
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database


def execution_for(instance_id: str) -> ExecutionManager:
    values = NousConfig(instance=InstanceConfig(id=instance_id, layout="portable"))
    snapshot = ConfigSnapshot(values, 1, FrozenValues({}))
    return ExecutionManager(ExecutionConfig(max_in_flight=1), lambda: snapshot)


class Maintenance:
    def __init__(self) -> None:
        self.mode = "error"
        self.started = asyncio.Event()
        self.cleaned = asyncio.Event()

    async def prepare_annotation(self, payload: JsonObject) -> JsonObject:
        return payload

    async def generate_annotation(self, plan: JsonObject) -> JsonObject:
        return plan

    async def adopt_annotation(self, value: JsonObject) -> JsonObject:
        return {"state": "ready"}

    async def refresh(self, payload: JsonObject) -> JsonObject:
        self.started.set()
        if self.mode == "error":
            raise ValueError("controlled failure")
        if self.mode == "wait":
            try:
                await asyncio.Event().wait()
            finally:
                await asyncio.sleep(0.1)
                self.cleaned.set()
        return {"state": "ready", "covered_generation": 1}


def test_transaction_retry_cancel_and_recovery(
    tmp_path: Path, caplog: pytest.LogCaptureFixture
) -> None:
    async def scenario() -> None:
        instance_id = str(uuid4())
        execution = execution_for(instance_id)
        settings = JobsConfig(poll_interval_seconds=0.1)
        stages = Maintenance()
        try:
            async with Database.open(
                tmp_path / "instance.sqlite", DatabaseConfig(), instance_id, create=True
            ) as database:
                await initialize_jobs(database, instance_id, settings)
                async with open_jobs(database, instance_id, settings, execution) as host:

                    async def enqueue() -> JobRef:
                        async with database.write() as tx:
                            return await host.operations.enqueue_in(
                                tx,
                                kind="refresh",
                                workflow_id=str(uuid4()),
                                payload={
                                    "namespace_ref": {"instance_id": instance_id, "id": "test"}
                                },
                            )

                    rolled_back = JobRef(instance_id=instance_id, workflow_id=str(uuid4()))
                    with pytest.raises(ValueError, match="rollback"):
                        async with database.write() as tx:
                            await host.operations.enqueue_in(
                                tx,
                                kind="refresh",
                                workflow_id=rolled_back.workflow_id,
                                payload={
                                    "namespace_ref": {"instance_id": instance_id, "id": "test"}
                                },
                            )
                            raise ValueError("rollback")
                    with pytest.raises(NousError, match="not found"):
                        await host.operations.read(rolled_back)

                    failed = await enqueue()
                    with pytest.raises(TimeoutError):
                        await host.operations.wait(failed, timeout=0.01)
                    assert (await host.operations.read(failed)).status == "ENQUEUED"
                    await host.start(stages)
                    with pytest.raises(NousError, match="controlled failure"):
                        await host.operations.wait(failed, timeout=10)
                    failed_view = await host.operations.read(failed)
                    assert failed_view.status == "ERROR" and failed_view.failed_step is not None
                    stages.mode = "ready"
                    retried = await host.operations.retry_failed(failed, failed_view.failed_step)
                    assert (await host.operations.wait(retried, timeout=10)).state == "ready"
                    old, new = await asyncio.gather(
                        host.operations.read(failed), host.operations.read(retried)
                    )
                    assert old.status == "ERROR" and new.forked_from == failed.workflow_id
                    assert old.queue_name == new.queue_name == "nous-refresh"

                    stages.mode = "wait"
                    stages.started.clear()
                    stages.cleaned.clear()
                    cancelled = await enqueue()
                    await asyncio.wait_for(stages.started.wait(), 10)
                    await host.operations.cancel(cancelled)
                    await asyncio.wait_for(stages.cleaned.wait(), 10)
                    assert (await host.operations.read(cancelled)).status == "CANCELLED"
                    stages.mode = "ready"
                    await host.operations.resume(cancelled)
                    assert (await host.operations.wait(cancelled, timeout=10)).state == "ready"
                    assert (await host.operations.read(cancelled)).queue_name == "nous-refresh"

                    stages.mode = "wait"
                    stages.started.clear()
                    stages.cleaned.clear()
                    interrupted = await enqueue()
                    await asyncio.wait_for(stages.started.wait(), 10)
                    await host.close(grace_seconds=0)
                    assert stages.cleaned.is_set()
                    assert (await host.operations.read(interrupted)).status == "PENDING"
                    stages.mode = "ready"
                    await host.start(stages)
                    assert (await host.operations.wait(interrupted, timeout=10)).state == "ready"
                    await host.close(grace_seconds=1)
                    pending = await enqueue()
                    waiting = asyncio.create_task(host.operations.wait(pending))
                    await asyncio.sleep(0.05)
                with pytest.raises(asyncio.CancelledError):
                    await waiting
        finally:
            await execution.aclose()

    asyncio.run(scenario())
    scheduler_failures = [
        record for record in caplog.records if record.name == "dbos" and record.levelname == "ERROR"
    ]
    assert scheduler_failures and all(record.exc_info is None for record in scheduler_failures)


def test_bridge_waits_for_business_cleanup_after_repeated_cancellation() -> None:
    async def scenario() -> None:
        execution = execution_for(str(uuid4()))
        bridge = RuntimeBridge(execution)
        started, finished = asyncio.Event(), asyncio.Event()
        dispatch: concurrent.futures.Future[
            tuple[asyncio.AbstractEventLoop, asyncio.Task[object]]
        ] = concurrent.futures.Future()

        async def stage(payload: JsonObject) -> JsonObject:
            started.set()
            try:
                await asyncio.Event().wait()
            finally:
                await asyncio.sleep(0.15)
                finished.set()
            return {}

        def scheduler() -> bool:
            async def call() -> bool:
                task = asyncio.current_task()
                assert task is not None
                dispatch.set_result((asyncio.get_running_loop(), task))
                try:
                    await bridge.call(stage, {}, association={"job_id": "test", "step": "test"})
                except asyncio.CancelledError:
                    return finished.is_set()
                return False

            return asyncio.run(call())

        worker = asyncio.create_task(asyncio.to_thread(scheduler))
        await asyncio.wait_for(started.wait(), 5)
        loop, task = dispatch.result()
        loop.call_soon_threadsafe(task.cancel)
        await asyncio.sleep(0.03)
        loop.call_soon_threadsafe(task.cancel)
        assert await asyncio.wait_for(worker, 5)
        assert finished.is_set() and bridge.in_flight == 0
        await bridge.quiesce(grace_seconds=0)
        await execution.aclose()

    asyncio.run(scenario())


def test_job_client_controls_active_host_without_another_instance_lock(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance # % 维护"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, workers=True, environment={}) as runtime:
            async with open_job_client(instance, environment={}) as jobs:
                assert jobs.instance_id == runtime.descriptor.instance_id
                values, cursor = await jobs.list()
                assert values == () and cursor is None

    asyncio.run(scenario())


def test_worker_refuses_an_existing_dbos_without_stopping_it(tmp_path: Path) -> None:
    instance = tmp_path / "instance"
    asyncio.run(initialize_instance(instance, environment={}))

    @DBOS.workflow()
    def external_work(value: int) -> int:
        return value + 1

    DBOS(
        config={
            "name": "external",
            "system_database_url": f"sqlite:///{(tmp_path / 'external.sqlite').as_posix()}",
            "enable_otlp": False,
        }
    )

    async def check_conflict() -> None:
        async with open_instance(instance, environment={}, workers=False):
            pass
        logger = logging.getLogger("dbos")
        before = (tuple(logger.handlers), logger.propagate, logger.level)
        with pytest.raises(NousError) as rejected:
            async with open_instance(instance, environment={}, workers=True):
                pass
        assert rejected.value.code == "job.host_in_use"
        assert (tuple(logger.handlers), logger.propagate, logger.level) == before

    try:
        asyncio.run(check_conflict())
        DBOS.launch()
        assert DBOS.start_workflow(external_work, 1).get_result() == 2
        asyncio.run(check_conflict())
        assert DBOS.start_workflow(external_work, 2).get_result() == 3
    finally:
        DBOS.destroy()


def test_existing_obsolete_business_schema_is_rejected_before_job_migration(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        database = instance / "data" / DatabaseConfig().filename
        with sqlite3.connect(database) as connection:
            connection.execute("ALTER TABLE processing_results DROP COLUMN adoption_signature")
            before = connection.execute("PRAGMA schema_version").fetchone()
        with pytest.raises(NousError) as rejected:
            await initialize_instance(instance, environment={})
        assert rejected.value.code == "storage.invalid_format"
        assert rejected.value.details["missing"] == {"processing_results": ["adoption_signature"]}
        with sqlite3.connect(database) as connection:
            assert connection.execute("PRAGMA schema_version").fetchone() == before

    asyncio.run(scenario())
