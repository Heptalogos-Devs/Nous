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
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError

from nous.platform.execution import DurableExecution, JobSpec
from nous.platform.persistence import Authority
from nous.platform.persistence.schema import clock, job_links


def context(caller="alice", namespace="workspace"):
    return CallContext(credential_source_ids=None, caller_id=caller, namespace_id=namespace)




def test_reader_snapshot_remains_stable_while_writer_commits(tmp_path):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        await authority.initialize()
        async with authority.read(context()) as before:
            assert before.commit_seq == 0
            assert (
                await before.connection.scalar(sa.select(sa.func.count()).select_from(clock))
                == 1
            )
        async with authority.read(context()) as after:
            assert after.commit_seq == 1
            with pytest.raises(sa.exc.OperationalError, match="readonly"):
                await after.connection.execute(clock.delete())
        await authority.close()

    asyncio.run(scenario())


def test_job_pages_filter_before_limit_and_bind_cursor_to_snapshot_and_scope(tmp_path):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        await authority.initialize()
        execution = DurableExecution(authority)

        async def echo(value):
            return {"value": value}

        execution.register("paged-echo", echo)
        execution.register("different-kind", echo)
        await execution.start()
        ctx = context()
        try:
            async with authority.write(ctx) as tx:
                for number in range(3):
                    await tx.enqueue(
                        JobSpec(
                            "paged-echo", "default", args=(number,), workflow_id=f"ready-{number}"
                        )
                    )
            for number in range(3):
                await asyncio.wait_for(execution.wait(ctx, f"ready-{number}"), timeout=10)
            async with authority.write(ctx) as tx:
                # More nonmatching rows than one internal query batch: older matches
                # must still fill the requested page.
                for number in range(130):
                    await tx.enqueue(
                        JobSpec(
                            "paged-echo",
                            "default",
                            args=(number,),
                            workflow_id=f"delayed-{number:03}",
                            delay_seconds=3600,
                        )
                    )
                await tx.enqueue(
                    JobSpec("different-kind", "default", args=(0,), workflow_id="other")
                )
            other = ctx.model_copy(update={"namespace_id": "foreign-workspace"})
            async with authority.write(other) as tx:
                await tx.enqueue(JobSpec("paged-echo", "default", args=(0,), workflow_id="foreign"))
            await asyncio.wait_for(execution.wait(other, "foreign"), timeout=10)
            page = await execution.query_jobs(ctx, kind="paged-echo", state="SUCCESS", limit=2)
            assert [job.workflow_id for job in page.items] == ["ready-2", "ready-1"]
            assert page.next_cursor
            async with authority.write(ctx) as tx:
                await tx.enqueue(JobSpec("paged-echo", "default", args=(0,), workflow_id="latest"))
            await asyncio.wait_for(execution.wait(ctx, "latest"), timeout=10)
            next_page = await execution.query_jobs(
                ctx, kind="paged-echo", state="SUCCESS", limit=2, cursor=page.next_cursor
            )
            assert [job.workflow_id for job in next_page.items] == ["ready-0"]
            assert next_page.next_cursor is None
            for modified in (
                other,
                ctx.model_copy(update={"caller_id": "another"}),
                ctx.model_copy(update={"namespace_id": "another"}),
            ):
                with pytest.raises(ValueError, match="cursor"):
                    await execution.query_jobs(
                        modified, kind="paged-echo", state="SUCCESS", cursor=page.next_cursor
                    )
            with pytest.raises(ValueError, match="cursor"):
                await execution.query_jobs(ctx, kind="paged-echo", cursor=page.next_cursor)
            with pytest.raises(ValueError, match="cursor"):
                await execution.query_jobs(ctx, cursor="invalid-cursor")
            assert await execution.list_jobs(context("another")) == []
        finally:
            await execution.close()
            await authority.close()

    asyncio.run(scenario())


def test_rollback_removes_commit_and_dbos_job(tmp_path):
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
                async with authority.write(context()) as tx:
                    ref = await tx.enqueue(
                        JobSpec("rollback-echo", "default", args=("durable-ref",))
                    )
                    raise RuntimeError("abort")
            async with authority.read(context()) as read:
                assert read.commit_seq == 0
                assert (
                    await read.connection.scalar(sa.select(sa.func.count()).select_from(job_links))
                    == 0
                )
            assert await execution.client.list_workflows_async(workflow_ids=[ref.workflow_id]) == []
            async with authority.write(context()) as tx:
                ref = await tx.enqueue(JobSpec("rollback-echo", "default", args=("durable-ref",)))
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
        async with authority.write(context()) as tx:
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
        await execution.cancel(context(), ref.workflow_id)
        assert (await execution.status(context(), ref.workflow_id)).status == "CANCELLED"
        assert (await execution.status(context(), owned.workflow_id)).status == "CANCELLED"
        assert (await execution.status(context(), shared.workflow_id)).status == "DELAYED"
        with pytest.raises(ValueError, match="cancelled"):
            async with authority.write(context()) as tx:
                await tx.enqueue(
                    JobSpec(
                        "restart-echo", "default", args=("late-ref",), owned_parent=ref.workflow_id
                    )
                )
        await execution.close()
        await authority.close()

        authority = Authority(path)
        await authority.initialize()
        execution = DurableExecution(authority)
        execution.register("restart-echo", echo)
        await execution.start()
        try:
            assert (await execution.status(context(), ref.workflow_id)).status == "CANCELLED"
            with pytest.raises(DomainError) as forbidden:
                await execution.resume(context("mallory"), ref.workflow_id)
            assert forbidden.value.code == "job_not_found"
            await execution.resume(context(), ref.workflow_id)
            async with asyncio.timeout(10):
                assert await execution.wait(context(), ref.workflow_id) == {
                    "reference": "durable-ref"
                }
        finally:
            await execution.close()
            await authority.close()

    asyncio.run(scenario())


def test_online_backup_restores_committed_wal(tmp_path):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        await authority.initialize()
        snapshot = await authority.backup(tmp_path / "backup.sqlite")
        with pytest.raises(FileExistsError):
            await authority.backup(snapshot)
        restored_path = await Authority.restore_backup(snapshot, tmp_path / "restored.sqlite")
        restored = Authority(restored_path)
        await restored.initialize()
        async with restored.read(context()) as view:
            assert view.commit_seq == 1
        async with restored.write(context()) as tx:
            assert tx.commit_seq == 2
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
    async with authority.write(context()) as tx:
        await tx.enqueue(
            JobSpec(
                "crash-recovery",
                "default",
                args=("persisted-request",),
                workflow_id="crash-recovery-job",
            )
        )
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
                async with authority.write(context()) as tx:
                    one = await tx.enqueue(JobSpec("host-use-case", "default", args=("one",)))
                    two = await tx.enqueue(JobSpec("host-use-case", "default", args=("two",)))
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
