"""Same-key serialization must not invert bounded execution admission."""

import asyncio
import json
import threading
import time
from http.server import BaseHTTPRequestHandler
from pathlib import Path

import pytest
from model_support import configure_models, model_server

from nous.content.contracts import OutputContent, SourceSelection
from nous.media.derivatives import MediaDerivatives
from nous.models.generation import GenerationRequest
from nous.models.requests import PreparedGeneration
from nous.runtime.bootstrap import open_instance


class _Handler(BaseHTTPRequestHandler):
    requests: list[bytes] = []
    slow_entered = threading.Event()
    slow_release = threading.Event()

    def do_POST(self) -> None:
        body = self.rfile.read(int(self.headers["Content-Length"]))
        self.requests.append(body)
        if b"HOLD_REQUEST" in body:
            self.slow_entered.set()
            if not self.slow_release.wait(15):
                return
        payload = json.dumps(
            {
                "id": "keyed-admission",
                "object": "chat.completion",
                "created": 1700000000,
                "model": "local-model",
                "choices": [
                    {
                        "index": 0,
                        "message": {"role": "assistant", "content": "Keyed output."},
                        "finish_reason": "stop",
                    }
                ],
            }
        ).encode()
        try:
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
        except BrokenPipeError, ConnectionResetError:
            pass

    def log_message(self, format: str, *args: object) -> None:
        pass


@pytest.mark.parametrize("api", ["prepared", "produce", "run"])
def test_nested_and_direct_same_key_do_not_invert_single_admission(
    tmp_path: Path, api: str
) -> None:
    _Handler.requests = []
    instance = tmp_path / "instance"
    with model_server(_Handler) as server:
        configure_models(instance, server)

        async def scenario() -> None:
            async with open_instance(
                instance,
                environment={},
                overrides={"execution.max_in_flight": 1, "execution.default_timeout_seconds": None},
            ) as runtime:
                snapshot = runtime.configuration.snapshot
                source = await runtime.artifacts.ingest("same-key source")
                selected = await runtime.generation.accept_inputs(source, snapshot)
                plan = await runtime.generation.prepare_input("read", selected, snapshot)
                prepared = await runtime.generation.prepare_group(
                    plan.frozen, plan.batches[0], snapshot
                )
                assert isinstance(prepared, PreparedGeneration)
                request = GenerationRequest("read", selected)
                admitted, release = asyncio.Event(), asyncio.Event()

                async def call() -> object:
                    if api == "prepared":
                        return await runtime.generation.produce_prepared(
                            prepared, producer_key="shared"
                        )
                    if api == "produce":
                        return await runtime.generation.produce(
                            request, computation_key="shared", snapshot=snapshot
                        )
                    return await runtime.generation.run(request, snapshot, computation_key="shared")

                async def nested() -> object:
                    async with runtime.execution.operation("composed", snapshot):
                        admitted.set()
                        await release.wait()
                        return await call()

                parent = asyncio.create_task(nested())
                direct: list[asyncio.Task[object]] = []
                try:
                    await asyncio.wait_for(admitted.wait(), 5)
                    direct = [asyncio.create_task(call()) for _ in range(8)]
                    for _ in range(200):
                        if runtime.execution.status()["queued"] == 1:
                            break
                        await asyncio.sleep(0.005)
                    assert runtime.execution.status()["queued"] == 1
                    release.set()
                    await asyncio.wait_for(asyncio.gather(parent, *direct), 8)
                    assert len(_Handler.requests) == 1
                finally:
                    release.set()
                    for task in (parent, *direct):
                        if not task.done():
                            task.cancel()
                    await asyncio.gather(parent, *direct, return_exceptions=True)

        asyncio.run(scenario())


@pytest.mark.parametrize("api", ["prepared", "produce", "run"])
def test_waiters_timeout_or_cancel_without_stalling_leader_or_other_keys(
    tmp_path: Path, api: str
) -> None:
    _Handler.requests = []
    _Handler.slow_entered = threading.Event()
    _Handler.slow_release = threading.Event()
    instance = tmp_path / "instance"
    with model_server(_Handler) as server:
        configure_models(instance, server)

        async def scenario() -> None:
            async with open_instance(
                instance,
                environment={},
                overrides={"execution.max_in_flight": 2, "execution.default_timeout_seconds": 0.5},
            ) as runtime:
                snapshot = runtime.configuration.snapshot
                source = await runtime.artifacts.ingest("timeout source")
                selected = await runtime.generation.accept_inputs(source, snapshot)

                async def prepare(question: str) -> PreparedGeneration:
                    plan = await runtime.generation.prepare_input(
                        "read", selected, snapshot, question=question
                    )
                    result = await runtime.generation.prepare_group(
                        plan.frozen, plan.batches[0], snapshot, question=question
                    )
                    assert isinstance(result, PreparedGeneration)
                    return result

                slow, fast = await prepare("HOLD_REQUEST"), await prepare("fast request")

                async def call(prepared: PreparedGeneration, key: str) -> object:
                    if api == "prepared":
                        return await runtime.generation.produce_prepared(prepared, producer_key=key)
                    request = GenerationRequest(
                        "read", selected, question=prepared.compiled.reading.question
                    )
                    if api == "produce":
                        return await runtime.generation.produce(
                            request, computation_key=key, snapshot=snapshot
                        )
                    return await runtime.generation.run(request, snapshot, computation_key=key)

                async def long_call(prepared: PreparedGeneration, key: str) -> object:
                    async with runtime.execution.operation(
                        "long", snapshot, deadline=asyncio.get_running_loop().time() + 10
                    ):
                        return await call(prepared, key)

                leader = asyncio.create_task(long_call(slow, "shared"))
                followers: list[asyncio.Task[object]] = []
                try:
                    assert await asyncio.to_thread(_Handler.slow_entered.wait, 5)
                    started = time.monotonic()
                    timeout_follower = asyncio.create_task(call(slow, "shared"))
                    followers.append(timeout_follower)
                    for _ in range(8):
                        followers.append(asyncio.create_task(call(slow, "shared")))
                    await asyncio.sleep(0.1)
                    assert runtime.execution.status()["running"] == 1
                    for follower in followers[1:]:
                        follower.cancel()
                    await asyncio.gather(*followers[1:], return_exceptions=True)
                    other = await asyncio.wait_for(long_call(fast, "other-key"), 4)
                    assert other is not None and not leader.done()
                    with pytest.raises(TimeoutError):
                        await timeout_follower
                    assert 0.4 <= time.monotonic() - started < 1.5
                    assert len(_Handler.requests) == 2
                    assert not leader.done()
                finally:
                    _Handler.slow_release.set()
                    await asyncio.gather(leader, *followers, return_exceptions=True)
                assert not leader.cancelled() and leader.exception() is None
                await call(slow, "shared")
                assert len(_Handler.requests) == 2

        asyncio.run(scenario())


def test_media_derivative_same_key_does_not_invert_admission(tmp_path: Path) -> None:
    from nous.runtime.bootstrap import initialize_instance

    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(
            instance,
            environment={},
            overrides={"execution.max_in_flight": 1, "execution.default_timeout_seconds": None},
        ) as runtime:
            source = await runtime.artifacts.ingest("source")
            derivatives = MediaDerivatives(runtime.artifacts, runtime.results)
            entered, release = asyncio.Event(), asyncio.Event()
            creations = 0

            async def create() -> dict[str, OutputContent]:
                nonlocal creations
                async with runtime.execution.operation("derivative.work"):
                    creations += 1
                    return {"output": OutputContent(data=b"derived", media_type="text/plain")}

            async def call() -> object:
                return await derivatives.get_or_create(
                    source, SourceSelection(), "test.derivative", {}, create
                )

            async def nested() -> object:
                async with runtime.execution.operation("composed"):
                    entered.set()
                    await release.wait()
                    return await call()

            parent = asyncio.create_task(nested())
            direct = None
            try:
                await asyncio.wait_for(entered.wait(), 5)
                direct = asyncio.create_task(call())
                await asyncio.sleep(0.1)
                release.set()
                await asyncio.wait_for(asyncio.gather(parent, direct), 5)
                assert creations == 1
            finally:
                release.set()
                for task in (parent, direct):
                    if task is not None and not task.done():
                        task.cancel()
                await asyncio.gather(
                    *(task for task in (parent, direct) if task is not None),
                    return_exceptions=True,
                )

    asyncio.run(scenario())


def test_key_wait_and_execution_share_one_deadline(tmp_path: Path) -> None:
    _Handler.requests = []
    _Handler.slow_entered = threading.Event()
    _Handler.slow_release = threading.Event()
    instance = tmp_path / "instance"
    with model_server(_Handler) as server:
        configure_models(instance, server)

        async def scenario() -> None:
            async with open_instance(
                instance,
                environment={},
                overrides={"execution.default_timeout_seconds": 0.7},
            ) as runtime:
                snapshot = runtime.configuration.snapshot
                source = await runtime.artifacts.ingest("deadline source")
                selected = await runtime.generation.accept_inputs(source, snapshot)
                plan = await runtime.generation.prepare_input(
                    "read", selected, snapshot, question="HOLD_REQUEST"
                )
                prepared = await runtime.generation.prepare_group(
                    plan.frozen, plan.batches[0], snapshot, question="HOLD_REQUEST"
                )
                assert isinstance(prepared, PreparedGeneration)
                lock = asyncio.Lock()
                runtime.generation._computation_locks["budget"] = lock  # pyright: ignore[reportPrivateUsage]
                started = time.monotonic()
                await lock.acquire()
                held_by_test = True
                task = asyncio.create_task(
                    runtime.generation.produce_prepared(prepared, producer_key="budget")
                )
                try:
                    await asyncio.sleep(0.4)
                    lock.release()
                    held_by_test = False
                    assert await asyncio.to_thread(_Handler.slow_entered.wait, 2)
                    with pytest.raises(TimeoutError):
                        await task
                    assert 0.6 <= time.monotonic() - started < 1.0
                    assert len(_Handler.requests) == 1
                finally:
                    if held_by_test:
                        lock.release()
                    _Handler.slow_release.set()
                    if not task.done():
                        task.cancel()
                        await asyncio.gather(task, return_exceptions=True)

        asyncio.run(scenario())


def test_shutdown_cancels_key_waiter_and_waits_for_owned_work(tmp_path: Path) -> None:
    from contextlib import AsyncExitStack

    _Handler.requests = []
    _Handler.slow_entered = threading.Event()
    _Handler.slow_release = threading.Event()
    instance = tmp_path / "instance"
    with model_server(_Handler) as server:
        configure_models(instance, server)

        async def scenario() -> None:
            async with AsyncExitStack() as stack:
                runtime = await stack.enter_async_context(
                    open_instance(
                        instance,
                        environment={},
                        overrides={
                            "execution.max_in_flight": 2,
                            "execution.default_timeout_seconds": None,
                            "runtime.shutdown_grace_seconds": 0.1,
                        },
                    )
                )
                snapshot = runtime.configuration.snapshot
                source = await runtime.artifacts.ingest("shutdown source")
                selected = await runtime.generation.accept_inputs(source, snapshot)
                plan = await runtime.generation.prepare_input(
                    "read", selected, snapshot, question="HOLD_REQUEST"
                )
                prepared = await runtime.generation.prepare_group(
                    plan.frozen, plan.batches[0], snapshot, question="HOLD_REQUEST"
                )
                assert isinstance(prepared, PreparedGeneration)
                leader = asyncio.create_task(
                    runtime.generation.produce_prepared(prepared, producer_key="shutdown")
                )
                follower = None
                try:
                    assert await asyncio.to_thread(_Handler.slow_entered.wait, 5)
                    follower = asyncio.create_task(
                        runtime.generation.produce_prepared(prepared, producer_key="shutdown")
                    )
                    await asyncio.sleep(0.1)
                    assert not follower.done()
                    await asyncio.wait_for(stack.aclose(), 5)
                    assert leader.done() and follower.done()
                    assert runtime.execution.in_flight == 0
                finally:
                    _Handler.slow_release.set()
                    await asyncio.gather(
                        *(task for task in (leader, follower) if task is not None),
                        return_exceptions=True,
                    )
            async with open_instance(instance, environment={}) as reopened:
                assert reopened.descriptor.instance_id == runtime.descriptor.instance_id

        asyncio.run(scenario())


def test_keyed_admission_many_waiters_cancel_and_release_without_starvation(tmp_path: Path) -> None:
    from nous.runtime.bootstrap import initialize_instance

    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(
            instance,
            environment={},
            overrides={"execution.max_in_flight": 2, "execution.default_timeout_seconds": None},
        ) as runtime:
            lock, other_lock = asyncio.Lock(), asyncio.Lock()
            started, release = asyncio.Event(), asyncio.Event()
            active = 0
            completed = 0

            async def work(first: bool = False) -> None:
                nonlocal active, completed
                async with runtime.execution.keyed_operation("test.key", lock):
                    active += 1
                    assert active == 1
                    try:
                        if first:
                            started.set()
                            await release.wait()
                        else:
                            await asyncio.sleep(0)
                        completed += 1
                    finally:
                        active -= 1

            leader = asyncio.create_task(work(first=True))
            followers: list[asyncio.Task[None]] = []
            try:
                await asyncio.wait_for(started.wait(), 5)
                followers = [asyncio.create_task(work()) for _ in range(64)]
                await asyncio.sleep(0.1)
                assert runtime.execution.status()["running"] == 1
                async with runtime.execution.keyed_operation("test.other", other_lock):
                    assert active == 1
                for task in followers[::2]:
                    task.cancel()
                await asyncio.gather(*followers[::2], return_exceptions=True)
                release.set()
                outcomes = await asyncio.wait_for(
                    asyncio.gather(leader, *followers, return_exceptions=True), 10
                )
                assert sum(isinstance(value, asyncio.CancelledError) for value in outcomes) == 32
                assert completed == 33
                assert not lock.locked()
                assert runtime.execution.in_flight == 0
            finally:
                release.set()
                for task in (leader, *followers):
                    if not task.done():
                        task.cancel()
                await asyncio.gather(leader, *followers, return_exceptions=True)

    asyncio.run(scenario())


@pytest.mark.parametrize("kind", ["resume_response", "recover_result"])
def test_recovery_waiter_times_out_without_holding_a_slot(tmp_path: Path, kind: str) -> None:
    _Handler.requests = []
    _Handler.slow_entered = threading.Event()
    _Handler.slow_release = threading.Event()
    instance = tmp_path / "instance"
    with model_server(_Handler) as server:
        configure_models(instance, server)

        async def scenario() -> None:
            async with open_instance(
                instance,
                environment={},
                overrides={"execution.max_in_flight": 2, "execution.default_timeout_seconds": 0.5},
            ) as runtime:
                snapshot = runtime.configuration.snapshot
                source = await runtime.artifacts.ingest("recover source")
                selected = await runtime.generation.accept_inputs(source, snapshot)
                plan = await runtime.generation.prepare_input(
                    "read", selected, snapshot, question="HOLD_REQUEST"
                )
                prepared = await runtime.generation.prepare_group(
                    plan.frozen, plan.batches[0], snapshot, question="HOLD_REQUEST"
                )
                assert isinstance(prepared, PreparedGeneration)
                request = GenerationRequest("read", selected, question="HOLD_REQUEST")

                async def produce() -> object:
                    async with runtime.execution.operation(
                        "long", snapshot, deadline=asyncio.get_running_loop().time() + 10
                    ):
                        if kind == "resume_response":
                            return await runtime.generation.produce_prepared(
                                prepared, producer_key="recover"
                            )
                        return await runtime.generation.run(
                            request, snapshot, computation_key="recover"
                        )

                async def recover() -> object:
                    if kind == "resume_response":
                        return await runtime.generation.resume_response(
                            "recover:response", snapshot
                        )
                    return await runtime.generation.recover_result("recover:response", snapshot)

                leader = asyncio.create_task(produce())
                follower = None
                try:
                    assert await asyncio.to_thread(_Handler.slow_entered.wait, 5)
                    follower = asyncio.create_task(recover())
                    await asyncio.sleep(0.1)
                    assert runtime.execution.status()["running"] == 1
                    with pytest.raises(TimeoutError):
                        await follower
                    assert not leader.done()
                finally:
                    _Handler.slow_release.set()
                    await asyncio.gather(
                        *(task for task in (leader, follower) if task is not None),
                        return_exceptions=True,
                    )
                assert leader.exception() is None
                assert await recover() is not None
                assert len(_Handler.requests) == 1

        asyncio.run(scenario())


def test_cancelled_leader_keeps_key_until_successful_response_is_durable(tmp_path: Path) -> None:
    from unittest.mock import patch

    _Handler.requests = []
    instance = tmp_path / "instance"
    with model_server(_Handler) as server:
        configure_models(instance, server)

        async def scenario() -> None:
            async with open_instance(
                instance,
                environment={},
                overrides={"execution.max_in_flight": 2, "execution.default_timeout_seconds": None},
            ) as runtime:
                snapshot = runtime.configuration.snapshot
                source = await runtime.artifacts.ingest("journal source")
                selected = await runtime.generation.accept_inputs(source, snapshot)
                plan = await runtime.generation.prepare_input("read", selected, snapshot)
                prepared = await runtime.generation.prepare_group(
                    plan.frozen, plan.batches[0], snapshot
                )
                assert isinstance(prepared, PreparedGeneration)
                files = runtime.generation.responses.journal.files
                original = files._save  # pyright: ignore[reportPrivateUsage]
                entered, release = threading.Event(), threading.Event()

                def delayed(key: str, value: object) -> None:
                    entered.set()
                    assert release.wait(10)
                    original(key, value)  # type: ignore[arg-type]

                with patch.object(files, "_save", side_effect=delayed):
                    leader = asyncio.create_task(
                        runtime.generation.produce_prepared(prepared, producer_key="journal")
                    )
                    follower = None
                    try:
                        assert await asyncio.to_thread(entered.wait, 5)
                        leader.cancel()
                        follower = asyncio.create_task(
                            runtime.generation.produce_prepared(prepared, producer_key="journal")
                        )
                        await asyncio.sleep(0.1)
                        assert not leader.done() and not follower.done()
                        assert runtime.execution.status()["running"] == 1
                    finally:
                        release.set()
                    with pytest.raises(asyncio.CancelledError):
                        await leader
                    assert follower is not None
                    result = await asyncio.wait_for(follower, 5)
                    assert result.outputs["output"].data == b"Keyed output."
                    assert len(_Handler.requests) == 1

        asyncio.run(scenario())


def test_nested_snapshot_conflict_is_rejected_before_key_wait(tmp_path: Path) -> None:
    from dataclasses import replace

    from nous.common.errors import NousError

    instance = tmp_path / "instance"
    with model_server(_Handler) as server:
        configure_models(instance, server)

        async def scenario() -> None:
            async with open_instance(instance, environment={}) as runtime:
                snapshot = runtime.configuration.snapshot
                source = await runtime.artifacts.ingest("snapshot source")
                selected = await runtime.generation.accept_inputs(source, snapshot)
                plan = await runtime.generation.prepare_input("read", selected, snapshot)
                prepared = await runtime.generation.prepare_group(
                    plan.frozen, plan.batches[0], snapshot
                )
                assert isinstance(prepared, PreparedGeneration)
                async with runtime.execution.operation("parent", snapshot):
                    await runtime.configuration.update({"logging.level": "INFO"})
                    changed = replace(prepared, snapshot=runtime.configuration.snapshot)
                    lock = asyncio.Lock()
                    runtime.generation._computation_locks["snapshot"] = lock  # pyright: ignore[reportPrivateUsage]
                    async with lock:
                        with pytest.raises(NousError) as failure:
                            await asyncio.wait_for(
                                runtime.generation.produce_prepared(
                                    changed, producer_key="snapshot"
                                ),
                                timeout=1,
                            )
                    assert failure.value.code == "operation.snapshot_conflict"

        asyncio.run(scenario())


def test_simultaneous_same_key_root_operations_all_make_progress(tmp_path: Path) -> None:
    from nous.runtime.bootstrap import initialize_instance

    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(
            instance,
            environment={},
            overrides={"execution.max_in_flight": 8, "execution.default_timeout_seconds": None},
        ) as runtime:
            lock = asyncio.Lock()
            release = asyncio.Event()
            entered: list[int] = []

            async def work(index: int) -> None:
                await release.wait()
                async with runtime.execution.keyed_operation("same-key", lock):
                    entered.append(index)
                    await asyncio.sleep(0)

            tasks = [asyncio.create_task(work(index)) for index in range(8)]
            try:
                release.set()
                await asyncio.wait_for(asyncio.gather(*tasks), 3)
                assert sorted(entered) == list(range(8))
                assert not lock.locked()
                assert runtime.execution.in_flight == 0
            finally:
                for task in tasks:
                    if not task.done():
                        task.cancel()
                await asyncio.gather(*tasks, return_exceptions=True)

    asyncio.run(scenario())


def test_idle_key_gates_do_not_retain_locks(tmp_path: Path) -> None:
    import gc
    import weakref

    from nous.runtime.bootstrap import initialize_instance

    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:

            async def use_key() -> weakref.ReferenceType[asyncio.Lock]:
                lock = asyncio.Lock()
                reference = weakref.ref(lock)
                async with runtime.execution.keyed_operation("temporary-key", lock):
                    pass
                return reference

            references = [await use_key() for _ in range(100)]
            gc.collect()
            assert all(reference() is None for reference in references)
            assert not runtime.execution._key_gates  # pyright: ignore[reportPrivateUsage]

    asyncio.run(scenario())


def test_timeout_during_response_journal_preserves_success_for_next_waiter(tmp_path: Path) -> None:
    from unittest.mock import patch

    _Handler.requests = []
    instance = tmp_path / "instance"
    with model_server(_Handler) as server:
        configure_models(instance, server)

        async def scenario() -> None:
            async with open_instance(
                instance,
                environment={},
                overrides={"execution.max_in_flight": 2, "execution.default_timeout_seconds": 0.5},
            ) as runtime:
                snapshot = runtime.configuration.snapshot
                source = await runtime.artifacts.ingest("timeout journal source")
                selected = await runtime.generation.accept_inputs(source, snapshot)
                plan = await runtime.generation.prepare_input("read", selected, snapshot)
                prepared = await runtime.generation.prepare_group(
                    plan.frozen, plan.batches[0], snapshot
                )
                assert isinstance(prepared, PreparedGeneration)
                files = runtime.generation.responses.journal.files
                original = files._save  # pyright: ignore[reportPrivateUsage]
                entered, release = threading.Event(), threading.Event()

                def delayed(key: str, value: object) -> None:
                    entered.set()
                    assert release.wait(10)
                    original(key, value)  # type: ignore[arg-type]

                async def follower() -> object:
                    async with runtime.execution.operation(
                        "long", snapshot, deadline=asyncio.get_running_loop().time() + 5
                    ):
                        return await runtime.generation.produce_prepared(
                            prepared, producer_key="timeout-journal"
                        )

                with patch.object(files, "_save", side_effect=delayed):
                    leader = asyncio.create_task(
                        runtime.generation.produce_prepared(
                            prepared, producer_key="timeout-journal"
                        )
                    )
                    next_call = None
                    try:
                        assert await asyncio.to_thread(entered.wait, 5)
                        await asyncio.sleep(0.6)
                        assert not leader.done()
                        next_call = asyncio.create_task(follower())
                        await asyncio.sleep(0.1)
                        assert not next_call.done()
                    finally:
                        release.set()
                    with pytest.raises(TimeoutError):
                        await leader
                    assert next_call is not None
                    assert await asyncio.wait_for(next_call, 5) is not None
                    assert len(_Handler.requests) == 1

        asyncio.run(scenario())
