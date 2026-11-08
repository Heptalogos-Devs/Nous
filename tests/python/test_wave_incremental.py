"""Incremental snapshot publication preserves old readers and current authority."""

import asyncio
from pathlib import Path
from typing import cast

import sqlalchemy as sa

from nous.cognition.retrieval.contracts import CognitiveQuery
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.wave import schema


def test_incremental_revision_deletion_and_borrowed_snapshot(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            cognition = await runtime.cognition()
            first = (await cognition.records.add_note("alpha retired shared", remember=True)).record
            second = (await cognition.records.add_note("beta shared prior", remember=True)).record
            await cognition.records.add_note("gamma shared unchanged", remember=True)
            fourth = (await cognition.records.add_note("theta original", remember=True)).record
            baseline = await cognition.retrieval.query(
                CognitiveQuery(text="shared", mode="lexical")
            )
            old_ref = baseline.context.snapshot_ref
            async with runtime.wave.snapshots.borrow(old_ref):

                async def read_old() -> None:
                    for _ in range(4):
                        result = await cognition.retrieval.query(
                            CognitiveQuery(
                                text="alpha", mode="lexical", snapshot=old_ref.identifier
                            )
                        )
                        assert any(
                            "alpha retired" in p.snippet for g in result.groups for p in g.passages
                        )
                        await asyncio.sleep(0)

                async def advance() -> None:
                    await cognition.records.archive(first.ref, expected=first.revision_ref)
                    await cognition.records.replace(
                        second.ref,
                        "beta shared prior",
                        expected=second.revision_ref,
                        title="deltaheading",
                    )
                    await cognition.records.replace(
                        fourth.ref, "delta shared revised", expected=fourth.revision_ref
                    )
                    fresh = await cognition.retrieval.query(
                        CognitiveQuery(text="shared", mode="lexical")
                    )
                    bodies = [p.snippet for group in fresh.groups for p in group.passages]
                    assert any("delta" in body for body in bodies)
                    assert any("beta shared prior" in body for body in bodies)
                    assert any("gamma shared unchanged" in body for body in bodies)
                    assert not any("alpha retired" in body for body in bodies)
                    title = await cognition.retrieval.query(
                        CognitiveQuery(text="deltaheading", mode="lexical")
                    )
                    assert any(
                        "title" in passage.matched_fields
                        for group in title.groups
                        for passage in group.passages
                    )
                    async with runtime.database.read() as tx:
                        summary = (
                            await tx.connection.execute(
                                sa.select(schema.snapshots.c.build_summary).where(
                                    schema.snapshots.c.id == fresh.context.snapshot_ref.id
                                )
                            )
                        ).scalar_one()
                    changes = summary["statistics"]["timings"]["lexical_delta"]
                    assert changes["incremental"] is True and changes["changed"] == 5
                    await runtime.wave.cleanup()
                    assert (await runtime.wave.describe(old_ref)).ref == old_ref

                await asyncio.gather(read_old(), advance())
            assert old_ref.identifier in cast(
                list[str], (await runtime.wave.cleanup())["snapshots"]
            )

    asyncio.run(scenario())


def test_incremental_build_recovers_after_process_crash(tmp_path: Path) -> None:
    import subprocess
    import sys

    instance = tmp_path / "crash-instance"

    async def seed() -> None:
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            cognition = await runtime.cognition()
            await cognition.records.add_note("stable original", remember=True)
            await cognition.retrieval.query(CognitiveQuery(text="stable", mode="lexical"))

    asyncio.run(seed())
    program = f"""
import asyncio, os
from pathlib import Path
from nous.runtime.bootstrap import open_instance
from nous.cognition.retrieval.contracts import CognitiveQuery
async def run():
    async with open_instance(Path({str(instance)!r}), environment={{}}) as runtime:
        cognition = await runtime.cognition()
        await cognition.records.add_note("pending crash recovery", remember=True)
        snapshots = runtime.wave.snapshots
        real = snapshots.engine()
        class CrashEngine:
            def open(self, *args, **kwargs): return real.open(*args, **kwargs)
            def close(self): real.close()
            def build(self, *args, **kwargs):
                path = Path(args[5])
                path.mkdir(parents=True)
                (path / "assets.msgpack").write_bytes(b"interrupted")
                os._exit(91)
        snapshots._engine = CrashEngine()
        await cognition.retrieval.query(CognitiveQuery(text="pending", mode="lexical"))
asyncio.run(run())
"""
    crashed = subprocess.run([sys.executable, "-c", program], capture_output=True, timeout=30)
    assert crashed.returncode == 91, crashed.stderr

    async def recover() -> None:
        async with open_instance(instance, environment={}) as runtime:
            cognition = await runtime.cognition()
            result = await cognition.retrieval.query(CognitiveQuery(text="pending", mode="lexical"))
            assert any(
                "pending crash recovery" in p.snippet for g in result.groups for p in g.passages
            )
            async with runtime.database.read() as tx:
                summary = (
                    await tx.connection.execute(
                        sa.select(schema.snapshots.c.build_summary).where(
                            schema.snapshots.c.id == result.context.snapshot_ref.id
                        )
                    )
                ).scalar_one()
            assert summary["statistics"]["timings"]["lexical_delta"]["incremental"] is True

    asyncio.run(recover())


def test_cancelled_incremental_build_keeps_base_borrowed_until_worker_exits(tmp_path: Path) -> None:
    import threading

    import pytest

    async def scenario() -> None:
        instance = tmp_path / "cancel-instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            cognition = await runtime.cognition()
            await cognition.records.add_note("old stable", remember=True)
            baseline = await cognition.retrieval.query(
                CognitiveQuery(text="stable", mode="lexical")
            )
            old_ref = baseline.context.snapshot_ref
            await cognition.records.add_note("new pending", remember=True)
            snapshots = runtime.wave.snapshots
            real = snapshots.engine()
            entered, release = threading.Event(), threading.Event()

            class PausedEngine:
                def build(self, *args: object, **kwargs: object) -> str:
                    entered.set()
                    if not release.wait(10):
                        raise TimeoutError("test did not release native worker")
                    return real.build(*args, **kwargs)  # type: ignore[arg-type]

                def open(self, *args: object, **kwargs: object) -> object:
                    return real.open(*args, **kwargs)  # type: ignore[arg-type]

                def close(self) -> None:
                    real.close()

            snapshots._engine = PausedEngine()  # type: ignore[assignment]  # pyright: ignore[reportPrivateUsage]
            task = asyncio.create_task(
                cognition.retrieval.query(CognitiveQuery(text="pending", mode="lexical"))
            )
            try:
                assert await asyncio.to_thread(entered.wait, 10)
                task.cancel()
                await asyncio.sleep(0.05)
                assert not task.done()
                await runtime.wave.cleanup()
                assert (await runtime.wave.describe(old_ref)).ref == old_ref
            finally:
                release.set()
            with pytest.raises(asyncio.CancelledError):
                await task
            snapshots._engine = real  # pyright: ignore[reportPrivateUsage]
            result = await cognition.retrieval.query(CognitiveQuery(text="pending", mode="lexical"))
            assert result.groups
            assert old_ref.identifier in cast(
                list[str], (await runtime.wave.cleanup())["snapshots"]
            )

    asyncio.run(scenario())


def test_keyed_build_locks_release_waiters_and_entries() -> None:
    import pytest

    from nous.common.keyed_locks import KeyedLocks

    async def scenario() -> None:
        pool = KeyedLocks[str]()
        waiting = asyncio.Event()

        async def blocked() -> None:
            waiting.set()
            async with pool.hold("same"):
                raise AssertionError("cancelled waiter must not enter")

        async with pool.hold("same"):
            task = asyncio.create_task(blocked())
            await waiting.wait()
            await asyncio.sleep(0)
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task
            assert len(pool._entries) == 1  # pyright: ignore[reportPrivateUsage]
        assert not pool._entries  # pyright: ignore[reportPrivateUsage]

    asyncio.run(scenario())


def test_missing_snapshot_query_during_rebuild_does_not_deadlock(tmp_path: Path) -> None:
    import shutil
    import threading

    import pytest

    from nous.common.errors import NousError
    from nous.wave.contracts import WaveQuery

    async def scenario() -> None:
        instance = tmp_path / "missing-component"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            async with runtime.execution.operation("test.wave"):
                view = await runtime.wave.snapshots.import_manifest(
                    Path("docs/examples/wave/collection.json")
                )
                profile = runtime.wave.resolve_build_profile(None)
                async with runtime.wave.prepare_build(view, profile, build_key="first") as first:
                    async with runtime.database.write() as tx:
                        first_ref = (await runtime.wave.accept_in(tx, first)).ref
                        await runtime.wave.promote_in(tx, first_ref)
                row = await runtime.wave.snapshots.resolve(first_ref)
                component = runtime.paths.resolve_owned("index", row["index_location"])
                changed = view.model_copy(
                    update={
                        "documents": (
                            view.documents[0].model_copy(update={"title": "Changed"}),
                            *view.documents[1:],
                        )
                    }
                )
                async with runtime.wave.prepare_build(
                    changed, profile, build_key="second"
                ) as second:
                    async with runtime.database.write() as tx:
                        descriptor = await runtime.wave.accept_in(tx, second)
                        await runtime.wave.promote_in(tx, descriptor.ref)
        shutil.rmtree(component)

        async with open_instance(instance, environment={}) as runtime:
            snapshots = runtime.wave.snapshots
            real = snapshots.engine()
            original_current = snapshots.reader.current
            base_selected, continue_build = asyncio.Event(), asyncio.Event()
            query_entered, continue_query = threading.Event(), threading.Event()

            async def current(name: str):
                result = await original_current(name)
                base_selected.set()
                await continue_build.wait()
                return result

            class PausedEngine:
                def open(self, path: str, *args: object, **kwargs: object):
                    if Path(path) == component:
                        query_entered.set()
                        if not continue_query.wait(10):
                            raise TimeoutError("query did not resume")
                    return real.open(path, *args, **kwargs)  # type: ignore[arg-type]

                def build(self, *args: object, **kwargs: object):
                    return real.build(*args, **kwargs)  # type: ignore[arg-type]

                def close(self) -> None:
                    real.close()

            snapshots.reader.current = current  # type: ignore[method-assign]
            snapshots._engine = PausedEngine()  # type: ignore[assignment]  # pyright: ignore[reportPrivateUsage]

            async def rebuild() -> None:
                async with runtime.wave.prepare_build(
                    view, profile, build_key="rebuild"
                ) as prepared:
                    async with runtime.database.write() as tx:
                        await runtime.wave.accept_in(tx, prepared)

            build_task = asyncio.create_task(rebuild())
            query_task: asyncio.Task[dict[str, object]] | None = None
            try:
                await asyncio.wait_for(base_selected.wait(), 10)
                query_task = asyncio.create_task(
                    runtime.wave.query(
                        WaveQuery(snapshot=first_ref.identifier, text="资源", mode="lexical")
                    )
                )
                assert await asyncio.to_thread(query_entered.wait, 5)
                continue_build.set()
                continue_query.set()
                with pytest.raises(NousError) as failure:
                    await asyncio.wait_for(query_task, 10)
                assert failure.value.code == "wave.snapshot_invalid"
                await asyncio.wait_for(build_task, 15)
                answer = await runtime.wave.query(
                    WaveQuery(snapshot=first_ref.identifier, text="资源", mode="lexical")
                )
                assert answer["hits"]
            finally:
                continue_build.set()
                continue_query.set()
                for task in (build_task, query_task):
                    if task is not None and not task.done():
                        task.cancel()
                await asyncio.gather(build_task, return_exceptions=True)
                if query_task is not None:
                    await asyncio.gather(query_task, return_exceptions=True)

    asyncio.run(scenario())


def test_process_killed_during_native_derivation_recovers(tmp_path: Path) -> None:
    import os
    import signal
    import subprocess
    import sys

    import pytest

    if os.name == "nt":
        pytest.skip("SIGKILL crash injection requires a POSIX process")
    instance, marker = tmp_path / "killed-instance", tmp_path / "killed-during-build"

    async def seed() -> None:
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            cognition = await runtime.cognition()
            for index in range(40):
                await cognition.records.add_note(f"base item {index} stable", remember=True)
            await cognition.retrieval.query(CognitiveQuery(text="stable", mode="lexical"))

    asyncio.run(seed())
    program = f"""
import asyncio, os, signal, threading, time
from pathlib import Path
from nous.runtime.bootstrap import open_instance
from nous.cognition.retrieval.contracts import CognitiveQuery
async def run():
    async with open_instance(Path({str(instance)!r}), environment={{}}) as runtime:
        cognition = await runtime.cognition()
        await cognition.records.add_note("actual crash pending", remember=True)
        snapshots = runtime.wave.snapshots
        real = snapshots.engine()
        class CrashEngine:
            def open(self, *args, **kwargs): return real.open(*args, **kwargs)
            def close(self): real.close()
            def build(self, *args, **kwargs):
                path = Path(args[5])
                def killer():
                    while not (path / "lexical").exists():
                        time.sleep(0.0001)
                    if not (path / "manifest.json").exists():
                        Path({str(marker)!r}).write_text("native build active")
                        os.kill(os.getpid(), signal.SIGKILL)
                threading.Thread(target=killer, daemon=True).start()
                return real.build(*args, **kwargs)
        snapshots._engine = CrashEngine()
        await cognition.retrieval.query(CognitiveQuery(text="pending", mode="lexical"))
asyncio.run(run())
"""
    crashed = subprocess.run([sys.executable, "-c", program], capture_output=True, timeout=30)
    assert crashed.returncode == -signal.SIGKILL and marker.exists(), crashed.stderr

    async def recover() -> None:
        async with open_instance(instance, environment={}) as runtime:
            cognition = await runtime.cognition()
            result = await cognition.retrieval.query(CognitiveQuery(text="pending", mode="lexical"))
            assert any(
                "actual crash pending" in p.snippet for g in result.groups for p in g.passages
            )
            async with runtime.database.read() as tx:
                summary = (
                    await tx.connection.execute(
                        sa.select(schema.snapshots.c.build_summary).where(
                            schema.snapshots.c.id == result.context.snapshot_ref.id
                        )
                    )
                ).scalar_one()
            assert summary["statistics"]["timings"]["lexical_delta"]["incremental"] is True

    asyncio.run(recover())
