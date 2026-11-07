"""Durable work through real instance and host entry points."""

import asyncio
import sqlite3
from pathlib import Path

import pytest
from dbos import DBOS
from pydantic import JsonValue

from nous.application.configuration import open_configuration_operations
from nous.cognition import AfterWrite
from nous.cognition.contracts import NamespaceRef
from nous.cognition.maintenance.contracts import MaintenanceOutcome, MaintenancePlan, RefreshRequest
from nous.cognition.maintenance.operations import EnqueuedMaintenance
from nous.cognition.writing.contracts import CommittedRecord, RecordOutcome
from nous.common.errors import NousError
from nous.configuration.definitions import DatabaseConfig
from nous.jobs.client import open_job_client
from nous.jobs.contracts import JobRef, JobView
from nous.runtime.bootstrap import Runtime, initialize_instance, open_instance
from nous.runtime.host import run_host
from nous.storage.database import WriteTx


def test_host_waits_without_slot_and_recovers_interrupted_work(tmp_path: Path) -> None:
    instance = tmp_path / "instance"

    async def setup() -> None:
        await initialize_instance(instance, environment={})
        async with open_configuration_operations(instance, environment={}) as config:
            await config.update(
                {
                    "execution.max_in_flight": 1,
                    "jobs.poll_interval_seconds": 0.1,
                    "runtime.shutdown_grace_seconds": 0.1,
                }
            )

    asyncio.run(setup())

    async def first_host(runtime: Runtime) -> tuple[JobRef, asyncio.Event]:
        cognition = await runtime.cognition()
        original = runtime.maintenance.refresh
        host_loop = asyncio.get_running_loop()
        started, release = asyncio.Event(), asyncio.Event()

        async def gated(
            namespace: NamespaceRef,
            minimum_generation: int,
            request: RefreshRequest,
            *,
            build_key: str,
        ) -> MaintenanceOutcome:
            assert asyncio.get_running_loop() is host_loop
            started.set()
            await release.wait()
            return await original(namespace, minimum_generation, request, build_key=build_key)

        runtime.maintenance.refresh = gated
        first = await cognition.records.create_journal(
            "Waiting for the actual maintenance operation.",
            after=AfterWrite(
                annotation="skip",
                retrieval="refresh",
                retrieval_mode="lexical",
                execution="background",
            ),
        )
        await asyncio.wait_for(started.wait(), 15)
        waiting = asyncio.create_task(runtime.jobs.wait(first.jobs[0], timeout=15))
        await asyncio.sleep(0.1)
        release.set()
        assert (await waiting)["state"] == "ready"

        policy = AfterWrite(
            annotation="skip",
            retrieval="refresh",
            retrieval_mode="lexical",
            execution="background",
        )
        edited = await cognition.records.replace(
            first.record.ref,
            "A revision with a stable edit identity.",
            expected=first.record.revision_ref,
            after=policy,
            write_key="edit-session",
        )
        assert (await runtime.jobs.wait(edited.jobs[0], timeout=20))["state"] == "ready"
        replayed = await cognition.records.replace(
            first.record.ref,
            "A revision with a stable edit identity.",
            expected=first.record.revision_ref,
            after=policy,
            write_key="edit-session",
        )
        assert replayed.record.revision_ref == edited.record.revision_ref
        assert not replayed.record.changed and replayed.retrieval.state == "ready"
        assert replayed.jobs == edited.jobs

        entered, cleaned = asyncio.Event(), asyncio.Event()

        async def interrupted(
            namespace: NamespaceRef,
            minimum_generation: int,
            request: RefreshRequest,
            *,
            build_key: str,
        ) -> MaintenanceOutcome:
            entered.set()
            try:
                await asyncio.Event().wait()
            finally:
                await asyncio.sleep(0.1)
                cleaned.set()
            return await original(namespace, minimum_generation, request, build_key=build_key)

        runtime.maintenance.refresh = interrupted
        second = await cognition.records.create_journal(
            "This maintenance must be recovered on restart.",
            after=AfterWrite(
                annotation="skip",
                retrieval="refresh",
                retrieval_mode="lexical",
                execution="background",
            ),
        )
        await asyncio.wait_for(entered.wait(), 15)
        with pytest.raises(TimeoutError):
            await runtime.jobs.wait(second.jobs[0], timeout=0.1)
        return second.jobs[0], cleaned

    ref, cleaned = run_host(instance, first_host, environment={})
    assert cleaned.is_set()

    async def inspect() -> JobView:
        async with open_job_client(instance, environment={}) as jobs:
            return await jobs.read(ref)

    assert asyncio.run(inspect()).status == "PENDING"

    async def next_host(runtime: Runtime) -> dict[str, JsonValue]:
        async with open_job_client(instance, environment={}) as client:
            assert client.instance_id == runtime.descriptor.instance_id
        recovered = await runtime.jobs.wait(ref, timeout=20)
        cognition = await runtime.cognition()
        original = runtime.maintenance.refresh
        entered, cleaned = asyncio.Event(), asyncio.Event()
        should_block = True

        async def cancellable(
            namespace: NamespaceRef,
            minimum_generation: int,
            request: RefreshRequest,
            *,
            build_key: str,
        ) -> MaintenanceOutcome:
            if should_block:
                entered.set()
                try:
                    await asyncio.Event().wait()
                finally:
                    cleaned.set()
            return await original(namespace, minimum_generation, request, build_key=build_key)

        runtime.maintenance.refresh = cancellable
        saved = await cognition.records.create_journal(
            "Explicit cancellation can be resumed.",
            after=AfterWrite(
                annotation="skip",
                retrieval="refresh",
                retrieval_mode="lexical",
                execution="background",
            ),
        )
        await asyncio.wait_for(entered.wait(), 15)
        cancelled = await runtime.jobs.cancel(saved.jobs[0])
        assert cancelled.status == "CANCELLED"
        await asyncio.wait_for(cleaned.wait(), 15)
        should_block = False
        await runtime.jobs.resume(saved.jobs[0])
        assert (await runtime.jobs.wait(saved.jobs[0], timeout=20))["state"] == "ready"
        runtime.maintenance.refresh = original
        private = await cognition.records.add_note(
            "A private note with an explicit refresh.",
            remember=False,
            after=AfterWrite(
                annotation="skip",
                retrieval="refresh",
                retrieval_mode="lexical",
                execution="background",
            ),
        )
        assert private.retrieval.state == "pending" and len(private.jobs) == 1
        assert (await runtime.jobs.wait(private.jobs[0], timeout=20))["state"] == "ready"
        return recovered

    assert run_host(instance, next_host, environment={})["state"] == "ready"


def test_accepted_save_can_enqueue_during_host_shutdown(tmp_path: Path) -> None:
    instance = tmp_path / "closing-save"

    async def setup() -> None:
        await initialize_instance(instance, environment={})
        async with open_configuration_operations(instance, environment={}) as config:
            await config.update({"runtime.shutdown_grace_seconds": 1})

    asyncio.run(setup())

    async def application(runtime: Runtime) -> asyncio.Task[RecordOutcome]:
        cognition = await runtime.cognition()
        enqueue = runtime.maintenance.enqueue_in
        writing, release_write = asyncio.Event(), asyncio.Event()

        async def held_write(
            tx: WriteTx, record: CommittedRecord, plan: MaintenancePlan
        ) -> EnqueuedMaintenance:
            writing.set()
            await release_write.wait()
            return await enqueue(tx, record, plan)

        runtime.maintenance.enqueue_in = held_write
        save = asyncio.create_task(
            cognition.records.create_journal(
                "An admitted save can enqueue during shutdown.",
                after=AfterWrite(
                    annotation="skip",
                    retrieval="refresh",
                    retrieval_mode="lexical",
                    execution="background",
                ),
            )
        )
        await asyncio.wait_for(writing.wait(), 15)
        asyncio.get_running_loop().call_later(0.05, release_write.set)
        return save

    save = run_host(instance, application, environment={})
    committed = save.result()
    assert committed.record.changed and len(committed.jobs) == 1

    async def inspect() -> JobView:
        async with open_job_client(instance, environment={}) as jobs:
            return await jobs.read(committed.jobs[0])

    assert asyncio.run(inspect()).status in {"ENQUEUED", "PENDING"}


def test_external_dbos_remains_owned_by_its_application(tmp_path: Path) -> None:
    instance = tmp_path / "instance"
    asyncio.run(initialize_instance(instance, environment={}))

    @DBOS.workflow(name="external.test")
    def external_work(value: int) -> int:
        return value + 1

    DBOS(
        config={
            "name": "external",
            "system_database_url": f"sqlite:///{(tmp_path / 'external.sqlite').as_posix()}",
            "enable_otlp": False,
        }
    )

    async def ordinary() -> None:
        async with open_instance(instance, environment={}):
            pass

    async def never_started(runtime: Runtime) -> None:
        raise AssertionError("The external DBOS owner must prevent startup")

    try:
        asyncio.run(ordinary())
        with pytest.raises(NousError) as rejected:
            run_host(instance, never_started, environment={})
        assert rejected.value.code == "job.host_in_use"
        DBOS.launch()
        assert DBOS.start_workflow(external_work, 1).get_result() == 2
        asyncio.run(ordinary())
        assert DBOS.start_workflow(external_work, 2).get_result() == 3
    finally:
        DBOS.destroy(destroy_registry=True)


def test_repeated_host_cancellation_finishes_sdk_shutdown(tmp_path: Path) -> None:
    instance = tmp_path / "cancel-host"
    asyncio.run(initialize_instance(instance, environment={}))
    owned: list[Runtime] = []

    async def interrupted(runtime: Runtime) -> None:
        owned.append(runtime)
        host_task = asyncio.current_task()
        assert host_task is not None

        async def cancel_twice() -> None:
            host_task.cancel()
            await asyncio.sleep(0.1)
            host_task.cancel()

        asyncio.create_task(cancel_twice())
        await asyncio.Event().wait()

    with pytest.raises(asyncio.CancelledError):
        run_host(instance, interrupted, environment={})
    assert not owned[0].job_runtime.worker_running

    async def started(runtime: Runtime) -> bool:
        return runtime.job_runtime.worker_running

    assert run_host(instance, started, environment={})


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
