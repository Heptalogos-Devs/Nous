"""Boundary failures: one SQLite commit owns both facts and durable work."""

import asyncio
import os
import sqlite3
import sys
from pathlib import Path

import httpx
import pytest
import sqlalchemy as sa
from dbos import DBOS
from nous_wave.contracts.common import CallContext

from nous.platform.execution import DurableExecution, JobSpec
from nous.platform.persistence import Authority, OperationConflict, OperationResultPurged
from nous.platform.persistence.schema import job_links, operations


def context(caller="alice", namespace="workspace"):
    return CallContext(caller_id=caller, namespace_id=namespace)


def test_operation_identity_survives_restart_and_serializes_concurrent_retries(tmp_path):
    async def scenario():
        path = tmp_path / "authority.sqlite"
        authority = Authority(path)
        await authority.initialize()
        executions = []

        async def accept(payload):
            async with authority.write(
                context(),
                "same-operation",
                operation_kind="accept",
                scope="subject",
                payload=payload,
            ) as tx:
                if tx.replayed:
                    return tx.receipt.result
                executions.append(tx.commit_seq)
                return tx.result({"accepted": payload, "seq": tx.commit_seq})

        result = await asyncio.gather(*(accept({"unit": "u1"}) for _ in range(8)))
        assert executions == [1]
        assert all(value == result[0] for value in result)
        with pytest.raises(OperationConflict):
            await accept({"unit": "different"})
        await authority.close()

        authority = Authority(path)
        await authority.initialize()
        assert await accept({"unit": "u1"}) == result[0]
        async with authority.write(
            context("bob"),
            "same-operation",
            operation_kind="accept",
            scope="subject",
            payload={"unit": "different"},
        ) as tx:
            assert tx.commit_seq == 2
            tx.result({"accepted": True})
        with sqlite3.connect(path) as connection:
            assert connection.execute("PRAGMA journal_mode").fetchone()[0] == "wal"
        await authority.close()

    asyncio.run(scenario())


def test_reader_snapshot_remains_stable_while_writer_commits(tmp_path):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        await authority.initialize()
        async with authority.read(context()) as before:
            assert before.commit_seq == 0
            async with authority.write(
                context(), "create", operation_kind="create", payload={}
            ) as tx:
                tx.result({"created": True})
            assert (
                await before.connection.scalar(sa.select(sa.func.count()).select_from(operations))
                == 0
            )
        async with authority.read(context()) as after:
            assert after.commit_seq == 1
            with pytest.raises(sa.exc.OperationalError, match="readonly"):
                await after.connection.execute(operations.delete())
        await authority.close()

    asyncio.run(scenario())


def test_rollback_removes_domain_changes_receipt_and_dbos_job(tmp_path):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        await authority.initialize()
        execution = DurableExecution(authority)

        class EchoWorker:
            async def echo(self, reference):
                return {"reference": reference}

        execution.register("rollback-echo", EchoWorker().echo)
        await execution.start()
        try:
            with pytest.raises(RuntimeError, match="abort"):
                async with authority.write(
                    context(), "rollback", operation_kind="adopt", payload={}
                ) as tx:
                    ref = await tx.enqueue(
                        JobSpec("rollback-echo", "default", args=("durable-ref",))
                    )
                    tx.result({"ref": ref.workflow_id})
                    raise RuntimeError("abort")
            async with authority.read(context()) as read:
                assert read.commit_seq == 0
                assert (
                    await read.connection.scalar(sa.select(sa.func.count()).select_from(job_links))
                    == 0
                )
                assert (
                    await read.connection.scalar(sa.select(sa.func.count()).select_from(operations))
                    == 0
                )
            assert await execution.client.list_workflows_async(workflow_ids=[ref.workflow_id]) == []
            async with authority.write(
                context(), "rollback", operation_kind="adopt", payload={}
            ) as tx:
                ref = await tx.enqueue(JobSpec("rollback-echo", "default", args=("durable-ref",)))
                tx.result({"ref": ref.workflow_id})
            async with asyncio.timeout(10):
                assert await execution.wait(context(), ref.workflow_id) == {
                    "reference": "durable-ref"
                }
        finally:
            await execution.close()
            # DBOS shutdown must leave the host's ordinary asyncio executor usable.
            assert await asyncio.to_thread(lambda: 42) == 42
            await authority.close()

    asyncio.run(scenario())


def test_cancel_and_resume_delayed_job_after_runtime_restart(tmp_path):
    async def scenario():
        path = tmp_path / "authority.sqlite"

        async def echo(reference):
            return {"reference": reference}

        authority = Authority(path)
        await authority.initialize()
        execution = DurableExecution(authority)
        execution.register("restart-echo", echo)
        await execution.start()
        async with authority.write(context(), "restart", operation_kind="adopt", payload={}) as tx:
            ref = await tx.enqueue(
                JobSpec("restart-echo", "default", args=("durable-ref",), delay_seconds=60)
            )
            owned = await tx.enqueue(
                JobSpec(
                    "restart-echo",
                    "default",
                    args=("owned-ref",),
                    delay_seconds=60,
                    owned_parent=ref.workflow_id,
                )
            )
            shared = await tx.enqueue(
                JobSpec("restart-echo", "default", args=("shared-ref",), delay_seconds=60)
            )
            tx.result({"job": ref.workflow_id})
        await execution.cancel(context(), ref.workflow_id)
        assert (await execution.status(context(), ref.workflow_id)).status == "CANCELLED"
        assert (await execution.status(context(), owned.workflow_id)).status == "CANCELLED"
        assert (await execution.status(context(), shared.workflow_id)).status == "DELAYED"
        with pytest.raises(ValueError, match="cancelled"):
            async with authority.write(
                context(), "late-child", operation_kind="adopt", payload={}
            ) as tx:
                await tx.enqueue(
                    JobSpec(
                        "restart-echo", "default", args=("late-ref",), owned_parent=ref.workflow_id
                    )
                )
                tx.result(None)
        await execution.close()
        await authority.close()

        authority = Authority(path)
        await authority.initialize()
        execution = DurableExecution(authority)
        execution.register("restart-echo", echo)
        await execution.start()
        try:
            assert (await execution.status(context(), ref.workflow_id)).status == "CANCELLED"
            with pytest.raises(LookupError):
                await execution.resume(context("mallory"), ref.workflow_id)
            await execution.resume(context(), ref.workflow_id)
            async with asyncio.timeout(10):
                assert await execution.wait(context(), ref.workflow_id) == {
                    "reference": "durable-ref"
                }
        finally:
            await execution.close()
            await authority.close()

    asyncio.run(scenario())


def test_online_backup_restores_committed_wal_and_receipts(tmp_path):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        await authority.initialize()
        async with authority.write(context(), "backup", operation_kind="create", payload={}) as tx:
            tx.result({"fact": "retained"})
        snapshot = await authority.backup(tmp_path / "backup.sqlite")
        with pytest.raises(FileExistsError):
            await authority.backup(snapshot)
        restored_path = await Authority.restore_backup(snapshot, tmp_path / "restored.sqlite")
        restored = Authority(restored_path)
        await restored.initialize()
        async with restored.write(context(), "backup", operation_kind="create", payload={}) as tx:
            assert tx.replayed
            assert tx.receipt.result == {"fact": "retained"}
        await restored.close()
        await authority.close()

    asyncio.run(scenario())


async def _recoverable_workflow(reference):
    await DBOS.set_event_async("started", True)
    await DBOS.sleep_async(3)
    return {"reference": reference}


async def _crash_worker(path):
    authority = Authority(path)
    await authority.initialize()
    execution = DurableExecution(authority)
    execution.register("crash-recovery", _recoverable_workflow)
    await execution.start()
    async with authority.write(context(), "crash", operation_kind="adopt", payload={}) as tx:
        await tx.enqueue(
            JobSpec(
                "crash-recovery",
                "default",
                args=("persisted-request",),
                workflow_id="crash-recovery-job",
            )
        )
        tx.result({"job": "crash-recovery-job"})
    async with asyncio.timeout(10):
        assert await execution.client.get_event_async("crash-recovery-job", "started") is True
        assert (await execution.status(context(), "crash-recovery-job")).status == "PENDING"
    os._exit(0)


def test_pending_workflow_recovers_after_process_exit(tmp_path):
    async def scenario():
        path = tmp_path / "authority.sqlite"
        environment = dict(os.environ)
        environment["PYTHONPATH"] = (
            str(Path(__file__).parent) + os.pathsep + environment.get("PYTHONPATH", "")
        )
        child = await asyncio.create_subprocess_exec(
            sys.executable,
            "-c",
            "import asyncio,sys; from test_transactions import _crash_worker; asyncio.run(_crash_worker(sys.argv[1]))",
            str(path),
            env=environment,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        try:
            async with asyncio.timeout(15):
                _, errors = await child.communicate()
            assert child.returncode == 0, errors.decode()
        finally:
            if child.returncode is None:
                child.kill()
                await child.wait()
        authority = Authority(path)
        await authority.initialize()
        execution = DurableExecution(authority)
        execution.register("crash-recovery", _recoverable_workflow)
        await execution.start()
        try:
            async with asyncio.timeout(10):
                assert await execution.wait(context(), "crash-recovery-job") == {
                    "reference": "persisted-request"
                }
            jobs = await execution.list_jobs(context())
            assert [(job.workflow_id, job.status) for job in jobs] == [
                ("crash-recovery-job", "SUCCESS")
            ]
            assert await execution.list_jobs(context("another-caller")) == []
            async with authority.read() as read:
                assert read.commit_seq == 1
        finally:
            await execution.close()
            await authority.close()

    asyncio.run(scenario())


def test_preemptible_use_case_keeps_shared_async_resources_on_host_loop(tmp_path):
    async def scenario():
        host = asyncio.get_running_loop()
        lock = asyncio.Lock()

        async def reply(reader, writer):
            await reader.readuntil(b"\r\n\r\n")
            writer.write(
                b'HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{"ok":true}'
            )
            await writer.drain()
            writer.close()
            await writer.wait_closed()

        server = await asyncio.start_server(reply, "127.0.0.1", 0)
        port = server.sockets[0].getsockname()[1]
        authority = Authority(tmp_path / "authority.sqlite")
        await authority.initialize()
        execution = DurableExecution(authority)
        async with server, httpx.AsyncClient() as client:

            class UseCase:
                async def run(self, reference):
                    assert asyncio.get_running_loop() is host
                    async with lock:
                        response = await client.get(f"http://127.0.0.1:{port}/")
                        async with authority.read() as view:
                            return {
                                "reference": reference,
                                "http": response.json(),
                                "seq": view.commit_seq,
                            }

            execution.register("host-use-case", UseCase().run, preemptible=True)
            await execution.start()
            try:
                async with authority.write(
                    context(), "host-use-case", operation_kind="run", payload={}
                ) as tx:
                    one = await tx.enqueue(JobSpec("host-use-case", "default", args=("one",)))
                    two = await tx.enqueue(JobSpec("host-use-case", "default", args=("two",)))
                    tx.result(None)
                async with asyncio.timeout(10):
                    values = await asyncio.gather(
                        execution.wait(context(), one.workflow_id),
                        execution.wait(context(), two.workflow_id),
                    )
                assert values == [
                    {"reference": "one", "http": {"ok": True}, "seq": 1},
                    {"reference": "two", "http": {"ok": True}, "seq": 1},
                ]
            finally:
                await execution.close()
        await authority.close()

    asyncio.run(scenario())


def test_source_receipt_redaction_keeps_operation_identity_and_prevents_reexecution(tmp_path):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        await authority.initialize()
        async with authority.write(
            context(), "paid", operation_kind="form", payload={"source": "source"}
        ) as tx:
            tx.result({"material_id": "source", "body": "private original text"})
        async with authority.write(context(), "purge", operation_kind="purge", payload={}) as tx:
            count = await authority.redact_receipts_in(
                tx, lambda value: isinstance(value, dict) and value.get("material_id") == "source"
            )
            assert count == 1
            tx.result({"purged": True})
        with pytest.raises(OperationResultPurged):
            await authority.receipt(
                context(), "paid", operation_kind="form", payload={"source": "source"}
            )
        with pytest.raises(OperationResultPurged):
            async with authority.write(
                context(), "paid", operation_kind="form", payload={"source": "source"}
            ):
                raise AssertionError("An erased completed operation must never execute again")
        with pytest.raises(OperationConflict):
            async with authority.write(
                context(), "paid", operation_kind="form", payload={"source": "changed"}
            ):
                raise AssertionError("A conflicting completed operation must never execute again")
        async with authority.read() as view:
            assert view.commit_seq == 2
            results = (
                (await view.connection.execute(sa.select(operations.c.result_json))).scalars().all()
            )
            assert all("private original text" not in value for value in results)
        await authority.close()

    asyncio.run(scenario())
