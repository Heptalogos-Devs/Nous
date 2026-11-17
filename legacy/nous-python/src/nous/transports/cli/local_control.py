"""Authenticated local calls and request-scoped delivery of caller inputs."""

from __future__ import annotations

import asyncio
import errno
import functools
import hmac
import inspect
import json
import math
import os
import secrets
import select
import shutil
import socket
import tempfile
import threading
import time
from collections.abc import AsyncGenerator, Awaitable, Callable, Mapping
from contextlib import asynccontextmanager
from pathlib import Path
from typing import TYPE_CHECKING, cast

from pydantic import TypeAdapter
from pydantic_core import to_jsonable_python

from nous.application.contracts import EventEnvelope, EventObserver
from nous.application.inputs import FileInput, InputProvider, input_chunks
from nous.common.errors import NousError, failure_code, failure_message
from nous.common.operations import exception_context
from nous.runtime.execution import complete_cleanup
from nous.runtime.instance import InstanceLocator
from nous.storage.file_input import FileReaderPool

from .control_wire import encode, endpoint, proof, read_endpoint, receive, receive_sync, send

if TYPE_CHECKING:
    from nous.runtime.bootstrap import Runtime


class LocalControl:
    """Own a local endpoint and every call connected to it."""

    def __init__(self, runtime: Runtime, locator: InstanceLocator) -> None:
        self.runtime, self.locator = runtime, locator
        self.server: asyncio.AbstractServer | None = None
        self.path: Path | None = None
        self.inode: int | None = None
        self.secret = secrets.token_hex(32)
        self.handlers: dict[asyncio.Task[None], asyncio.StreamWriter] = {}
        self.handshakes: dict[asyncio.Task[None], asyncio.StreamWriter] = {}
        self.slots = asyncio.Semaphore(16)
        self.wait_slots = asyncio.Semaphore(32)
        self.control_slots = asyncio.Semaphore(8)
        self.authorized_waiters = 0

    async def start(self) -> None:
        path = endpoint(self.locator, create=True)
        if path is None:
            return
        read_endpoint(path)
        for stale in path.parent.glob(f".{path.stem}.input-*"):
            if stale.is_dir() and not stale.is_symlink():
                await asyncio.to_thread(shutil.rmtree, stale, ignore_errors=True)
        self.server = await asyncio.start_server(self.accept, host="127.0.0.1", port=0)
        address = self.server.sockets[0].getsockname()
        temporary = path.with_name(f".{path.name}.{secrets.token_hex(8)}")
        try:
            secret = self.secret
            if os.name == "nt":
                from .control_windows import protect

                secret = protect(secret)
            descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
                json.dump({"host": "127.0.0.1", "port": address[1], "secret": secret}, stream)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, path)
        except BaseException:
            temporary.unlink(missing_ok=True)
            self.server.close()
            await self.server.wait_closed()
            raise
        self.path, self.inode = path, path.lstat().st_ino

    def accept(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        if len(self.handshakes) >= 32:
            oldest = next(iter(self.handshakes))
            self.handshakes.pop(oldest).close()
            oldest.cancel()
        task = asyncio.create_task(self.handle(reader, writer))
        self.handshakes[task] = writer
        self.handlers[task] = writer

        def finished(done: asyncio.Task[None]) -> None:
            self.handshakes.pop(done, None)
            self.handlers.pop(done, None)

        task.add_done_callback(finished)

    async def _authenticate(
        self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter
    ) -> None:
        peer = writer.get_extra_info("peername")
        if not peer or peer[0] != "127.0.0.1":
            raise ValueError("Local control accepts loopback peers only")
        async with asyncio.timeout(5):
            hello = await receive(reader, limit=4096)
            nonce = hello.get("nonce")
            if hello.get("kind") != "hello" or not isinstance(nonce, str) or len(nonce) != 64:
                raise ValueError("Invalid local control handshake")
            server_nonce = secrets.token_hex(32)
            await send(
                writer,
                {
                    "kind": "hello",
                    "nonce": server_nonce,
                    "proof": proof(self.secret, "server", nonce),
                },
            )
            auth = await receive(reader, limit=4096)
            client_proof = auth.get("proof")
            if (
                auth.get("kind") != "auth"
                or not isinstance(client_proof, str)
                or not hmac.compare_digest(
                    client_proof, proof(self.secret, "client", nonce, server_nonce)
                )
            ):
                raise ValueError("Local control authentication failed")

    async def handle(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        task = asyncio.current_task()
        assert task is not None
        try:
            await self._authenticate(reader, writer)
            request = await asyncio.wait_for(receive(reader), 600)
            try:
                from nous.runtime.operations import (
                    CANCEL_OPERATION,
                    LIST_OPERATIONS,
                    READ_OPERATION,
                    WAIT_OPERATION,
                )

                name = request.get("operation")
                timeout = request.get("timeout")
                if timeout is not None and (
                    isinstance(timeout, bool)
                    or not isinstance(timeout, (int, float))
                    or not math.isfinite(timeout)
                    or timeout <= 0
                ):
                    raise ValueError("Invalid application request timeout")
                label = (
                    name
                    if isinstance(name, str)
                    else (
                        "application.catalog"
                        if request.get("kind") == "catalog"
                        else "content.deliver"
                    )
                )
                slots = (
                    self.wait_slots
                    if name == WAIT_OPERATION.name
                    else self.control_slots
                    if name in {CANCEL_OPERATION.name, LIST_OPERATIONS.name, READ_OPERATION.name}
                    else self.slots
                )
                # Admission and external input transfer share the root deadline.
                async with self.runtime.execution.request(
                    label, timeout=cast(float | None, timeout)
                ):
                    self.handshakes.pop(task, None)
                    if self.authorized_waiters >= 64:
                        raise NousError("operation.busy", "Too many waiting local requests")
                    self.authorized_waiters += 1
                    try:
                        await slots.acquire()
                    finally:
                        self.authorized_waiters -= 1
                    try:
                        if request.get("kind") == "catalog":
                            selected = request.get("name")
                            if selected is None:
                                result = {"operations": self.runtime.app.list()}
                            elif isinstance(selected, str):
                                result = self.runtime.app.describe(selected)
                            else:
                                raise ValueError("Invalid application operation name")
                        elif request.get("kind") == "export":
                            result = await self._deliver_artifact(reader, writer, request)
                        else:
                            result = await self._call(reader, writer, request)
                    finally:
                        slots.release()
                    await send(
                        writer,
                        {"kind": "result", "value": to_jsonable_python(result, round_trip=True)},
                    )
            except Exception as error:
                failure = (
                    error
                    if isinstance(error, NousError)
                    else NousError(
                        failure_code(error),
                        failure_message(error),
                        details=exception_context(error),
                    )
                )
                await send(writer, {"kind": "result", "error": failure.to_dict()})
        except Exception, asyncio.CancelledError:
            pass
        finally:
            writer.close()
            try:
                await writer.wait_closed()
            except OSError:
                pass

    async def _call(
        self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter, request: dict[str, object]
    ) -> object:
        name = request.get("operation")
        timeout = request.get("timeout")
        if not isinstance(name, str) or (
            timeout is not None and (not isinstance(timeout, (int, float)) or timeout < 0)
        ):
            raise ValueError("Invalid application request")
        deferred = request.get("prepare") is True
        payload = self.runtime.app.parse(name, request.get("request"))
        async with self.runtime.execution.request(
            name, timeout=cast(float | None, timeout)
        ) as execution:
            if deferred:
                remaining = (
                    max(0.0, execution.deadline - asyncio.get_running_loop().time())
                    if execution.deadline is not None
                    else None
                )
                await send(
                    writer,
                    {
                        "kind": "accepted",
                        "timeout": remaining,
                        "text_limit": execution.settings.values.content.text_read_max_bytes,
                    },
                )
                request = await receive(reader)
                if request.get("kind") != "prepared":
                    raise ValueError("Invalid prepared application request")
                payload = self.runtime.app.parse(name, request.get("request"))
            declared = request.get("inputs")
            if not isinstance(declared, dict):
                raise ValueError("Invalid application inputs")
            declarations = cast(dict[object, object], declared)
            assert self.path is not None
            staging = Path(
                tempfile.mkdtemp(prefix=f".{self.path.stem}.input-", dir=self.path.parent)
            )
            try:
                bindings: dict[str, InputProvider] = {}
                paths: dict[str, str] = {}
                for key, metadata in declarations.items():
                    if not isinstance(key, str) or not key or not isinstance(metadata, dict):
                        raise ValueError("Invalid external input declaration")
                    values = cast(dict[str, object], metadata)
                    input_name, source = values.get("name"), values.get("source")
                    if not isinstance(input_name, str) or (
                        source is not None and not isinstance(source, str)
                    ):
                        raise ValueError("Invalid external input description")
                    target = staging / secrets.token_hex(16)
                    bindings[key] = FileInput(target, name=input_name, source=source or input_name)
                    paths[key] = str(target)
                remaining = (
                    max(0.0, execution.deadline - asyncio.get_running_loop().time())
                    if execution.deadline is not None
                    else None
                )
                await send(writer, {"kind": "bindings", "paths": paths, "timeout": remaining})
                ready = await receive(reader)
                if ready.get("kind") != "bindings_ready":
                    raise ValueError("External input delivery was not completed")
                if await asyncio.to_thread(_delivery_incomplete, paths):
                    raise NousError("input.incomplete", "External input delivery is incomplete")

                async def observe(event: EventEnvelope) -> None:
                    await send(writer, {"kind": "event", "event": event.model_dump(mode="json")})

                worker = asyncio.create_task(
                    self.runtime.app.call_name(name, payload, inputs=bindings, observer=observe)
                )
                incoming = asyncio.create_task(receive(reader))
                try:
                    done, _ = await asyncio.wait(
                        (worker, incoming), return_when=asyncio.FIRST_COMPLETED
                    )
                    if incoming in done:
                        # A client disconnect or any unsolicited frame ends the foreground call.
                        incoming.result()
                        raise ValueError("Unexpected local control message")
                    return worker.result()
                finally:
                    for pending in (worker, incoming):
                        if not pending.done():
                            pending.cancel()
                    await asyncio.gather(worker, incoming, return_exceptions=True)
            finally:
                await asyncio.to_thread(shutil.rmtree, staging, ignore_errors=True)

    async def _deliver_artifact(
        self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter, request: dict[str, object]
    ) -> dict[str, object]:
        from nous.content.contracts import ArtifactRef

        incoming = request.get("artifact")
        ref = incoming if isinstance(incoming, str) else ArtifactRef.model_validate(incoming)
        timeout = request.get("timeout")
        if timeout is not None and (not isinstance(timeout, (int, float)) or timeout <= 0):
            raise ValueError("Invalid delivery timeout")
        async with self.runtime.execution.request(
            "content.deliver", timeout=cast(float | None, timeout)
        ) as context:
            blob = await self.runtime.artifacts.blob(ref)
            async with self.runtime.artifacts.content.borrow_local(blob) as source:
                remaining = (
                    max(0.0, context.deadline - asyncio.get_running_loop().time())
                    if context.deadline is not None
                    else None
                )
                await send(
                    writer,
                    {
                        "kind": "export_source",
                        "source": str(source),
                        "digest": blob.digest,
                        "length": blob.byte_length,
                        "timeout": remaining,
                    },
                )
                if (await receive(reader)).get("kind") != "export_done":
                    raise ValueError("Artifact delivery was not completed")
        return {"delivered": True}

    async def stop(self) -> None:
        if self.server is not None:
            self.server.close()
            await self.server.wait_closed()
        self.runtime.execution.stop_admission()
        for task, writer in list(self.handshakes.items()):
            writer.close()
            task.cancel()
        if self.handshakes:
            await asyncio.gather(*list(self.handshakes), return_exceptions=True)
        if self.path is not None:
            try:
                if self.path.lstat().st_ino == self.inode:
                    self.path.unlink()
            except FileNotFoundError:
                pass


@asynccontextmanager
async def serve_cli(runtime: Runtime, locator: InstanceLocator) -> AsyncGenerator[None]:
    control = LocalControl(runtime, locator)
    await control.start()
    try:
        yield
    finally:
        await complete_cleanup(control.stop())


def _delivery_incomplete(paths: Mapping[str, str]) -> bool:
    return any(not Path(path).is_file() or Path(path).is_symlink() for path in paths.values())


def _validate_delivery_paths(locator: InstanceLocator, paths: Mapping[str, str]) -> None:
    endpoint_path = endpoint(locator)
    if endpoint_path is None:
        raise ValueError("Local endpoint is unavailable")
    parents: set[Path] = set()
    targets: set[Path] = set()
    for value in paths.values():
        target = Path(value)
        directory = target.parent
        if (
            not target.is_absolute()
            or directory.parent != endpoint_path.parent
            or not directory.name.startswith(f".{endpoint_path.stem}.input-")
            or directory.is_symlink()
            or directory.is_junction()
            or len(target.name) != 32
            or any(character not in "0123456789abcdef" for character in target.name)
        ):
            raise ValueError("Invalid local input path")
        parents.add(directory)
        targets.add(target)
    if len(parents) > 1 or len(targets) != len(paths):
        raise ValueError("Invalid local input session")


def _discard_incomplete_delivery(paths: Mapping[str, str]) -> None:
    directories: set[Path] = set()
    for value in paths.values():
        target = Path(value)
        directories.add(target.parent)
        for file in (target, target.with_suffix(".part")):
            try:
                file.unlink(missing_ok=True)
            except OSError:
                pass
    for directory in directories:
        try:
            directory.rmdir()
        except OSError:
            pass


async def _deliver_inputs(
    bindings: Mapping[str, InputProvider], paths: Mapping[str, str], timeout_seconds: float | None
) -> None:
    pool = FileReaderPool(1)
    completed = False
    try:
        async with asyncio.timeout(timeout_seconds):
            for key, source in bindings.items():
                target = Path(paths[key])
                temporary = target.with_suffix(".part")
                try:
                    with temporary.open("xb") as output:
                        chunks = input_chunks(source, pool, 1024 * 1024)
                        try:
                            async for chunk in chunks:
                                await _client_blocking(output.write, chunk)
                        finally:
                            await chunks.aclose()
                        await _client_blocking(output.flush)
                        await _client_blocking(os.fsync, output.fileno())
                    await _client_blocking(os.replace, temporary, target)
                finally:
                    temporary.unlink(missing_ok=True)
        completed = True
    finally:
        try:
            await pool.aclose()
        finally:
            if not completed:
                _discard_incomplete_delivery(paths)


async def _client_blocking[T](action: Callable[..., T], *args: object) -> T:
    """Keep client-owned files and iterators alive until blocking work has exited."""
    return await complete_cleanup(asyncio.to_thread(action, *args))


class _HostResponse(Exception):
    """The host sent a response while the client was preparing input."""


async def _client_work[T](
    work: Awaitable[T], sock: socket.socket, timeout_seconds: float | None
) -> T:
    async def watch_connection() -> None:
        while True:
            await asyncio.sleep(0.1)
            ready, _, _ = select.select([sock], [], [], 0)
            if ready:
                if not sock.recv(1, socket.MSG_PEEK):
                    raise EOFError("Local host connection closed")
                raise _HostResponse

    task = asyncio.ensure_future(work)
    watcher = asyncio.create_task(watch_connection())
    try:
        async with asyncio.timeout(timeout_seconds):
            done, _ = await asyncio.wait((task, watcher), return_when=asyncio.FIRST_COMPLETED)
            if watcher in done:
                watcher.result()
            return task.result()
    finally:
        for pending in (task, watcher):
            if not pending.done():
                pending.cancel()
        await asyncio.gather(task, watcher, return_exceptions=True)


def _connect(locator: InstanceLocator) -> socket.socket | None:
    path = endpoint(locator)
    info = read_endpoint(path) if path is not None else None
    if info is None:
        return None
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(10)
    try:
        try:
            sock.connect(("127.0.0.1", info["port"]))
        except OSError as error:
            if error.errno in {errno.ENOENT, errno.ECONNREFUSED}:
                sock.close()
                return None
            raise
        nonce = secrets.token_hex(32)
        sock.sendall(encode({"kind": "hello", "nonce": nonce}))
        hello = receive_sync(sock)
        server_nonce, server_proof = hello.get("nonce"), hello.get("proof")
        if (
            hello.get("kind") != "hello"
            or not isinstance(server_nonce, str)
            or len(server_nonce) != 64
            or not isinstance(server_proof, str)
            or not hmac.compare_digest(server_proof, proof(info["secret"], "server", nonce))
        ):
            raise NousError(
                "instance.control_unavailable", "Cannot authenticate the local instance host"
            )
        sock.sendall(
            encode({"kind": "auth", "proof": proof(info["secret"], "client", nonce, server_nonce)})
        )
        return sock
    except BaseException:
        sock.close()
        raise


def request_catalog(locator: InstanceLocator, name: str | None) -> dict[str, object] | None:
    sock = _connect(locator)
    if sock is None:
        return None
    try:
        sock.sendall(encode({"kind": "catalog", "name": name}))
        response = _check_result(receive_sync(sock))
        return TypeAdapter(dict[str, object]).validate_python(response.get("value"))
    finally:
        sock.close()


def request_call(
    locator: InstanceLocator,
    name: str,
    request: object,
    *,
    inputs: Mapping[str, InputProvider] | None = None,
    observer: EventObserver | None = None,
    timeout: float | None = None,
    prepare: Callable[
        [object, Mapping[str, InputProvider], int],
        Awaitable[tuple[object, Mapping[str, InputProvider]]],
    ]
    | None = None,
) -> dict[str, object] | None:
    """Deliver local inputs before invoking the same registered operation in the host."""
    bindings = dict(inputs or {})

    def declarations(values: Mapping[str, InputProvider]) -> dict[str, dict[str, str | None]]:
        declared: dict[str, dict[str, str | None]] = {}
        for key, value in values.items():
            if isinstance(value, FileInput):
                declared[key] = {
                    "name": value.name or value.path.name,
                    "source": value.source or str(value.path),
                }
            else:
                declared[key] = {"name": value.name, "source": value.source}
        return declared

    sock = _connect(locator)
    if sock is None:
        return None
    runner = asyncio.Runner()
    deadline: float | None = None
    try:
        if prepare is not None:
            sock.sendall(
                encode(
                    {
                        "operation": name,
                        "request": to_jsonable_python(request, round_trip=True),
                        "prepare": True,
                        "timeout": timeout,
                    }
                )
            )
            accepted = receive_sync(sock)
            if accepted.get("kind") == "result":
                return _check_result(accepted)
            if accepted.get("kind") != "accepted":
                raise ValueError("Invalid local request acceptance")
            budget = TypeAdapter(float | None).validate_python(accepted.get("timeout"))
            limit = TypeAdapter(int).validate_python(accepted.get("text_limit"))
            deadline = time.monotonic() + budget if budget is not None else None
            request, prepared = runner.run(
                _client_work(prepare(request, bindings, limit), sock, budget)
            )
            bindings = dict(prepared)
            sock.sendall(
                encode(
                    {
                        "kind": "prepared",
                        "request": to_jsonable_python(request, round_trip=True),
                        "inputs": declarations(bindings),
                    }
                )
            )
        else:
            sock.sendall(
                encode(
                    {
                        "operation": name,
                        "request": to_jsonable_python(request, round_trip=True),
                        "inputs": declarations(bindings),
                        "timeout": timeout,
                    }
                )
            )
        reply = receive_sync(sock)
        if reply.get("kind") == "result":
            return _check_result(reply)
        if reply.get("kind") != "bindings":
            raise ValueError("Invalid local input delivery")
        try:
            delivered = TypeAdapter(dict[str, str]).validate_python(reply.get("paths"))
            budget = TypeAdapter(float | None).validate_python(reply.get("timeout"))
        except ValueError as error:
            raise ValueError("Invalid local input paths") from error
        if set(delivered) != set(bindings):
            raise ValueError("Invalid local input paths")
        _validate_delivery_paths(locator, delivered)
        deadline = time.monotonic() + budget if budget is not None else None
        runner.run(_client_work(_deliver_inputs(bindings, delivered, budget), sock, budget))
        sock.sendall(encode({"kind": "bindings_ready"}))
        sock.settimeout(None)
        while True:
            reply = receive_sync(sock)
            if reply.get("kind") == "event":
                event = EventEnvelope.model_validate(reply.get("event"))
                if observer is not None:
                    result = observer(event)
                    if inspect.isawaitable(result):
                        runner.run(result)
                continue
            if reply.get("kind") != "result":
                raise ValueError("Invalid local host response")
            return _check_result(reply)
    except _HostResponse:
        return _check_result(receive_sync(sock))
    except TimeoutError:
        raise
    except (EOFError, OSError, ValueError) as error:
        if deadline is not None and time.monotonic() >= deadline:
            raise TimeoutError("Operation exceeded its deadline") from error
        raise NousError(
            "instance.control_lost", "The local host connection was lost", cause=error
        ) from error
    finally:
        sock.close()
        runner.close()


def request_export(
    locator: InstanceLocator, ref: object, destination: Path, *, timeout: float | None = None
) -> bool:
    """Copy an artifact while the running host holds its immutable content lease."""
    from nous.storage.file_output import deliver_file

    sock = _connect(locator)
    if sock is None:
        return False
    try:
        sock.sendall(
            encode(
                {
                    "kind": "export",
                    "artifact": to_jsonable_python(ref, round_trip=True),
                    "timeout": timeout,
                }
            )
        )
        reply = receive_sync(sock)
        if reply.get("kind") == "result":
            _check_result(reply)
            raise ValueError("Missing artifact delivery")
        source, digest, length, remaining = (
            reply.get("source"),
            reply.get("digest"),
            reply.get("length"),
            reply.get("timeout"),
        )
        if (
            reply.get("kind") != "export_source"
            or not isinstance(source, str)
            or not Path(source).is_absolute()
            or not isinstance(digest, str)
            or not isinstance(length, int)
        ):
            raise ValueError("Invalid artifact delivery")

        async def deliver() -> None:
            cancellation = threading.Event()
            future = asyncio.get_running_loop().run_in_executor(
                None,
                functools.partial(
                    deliver_file,
                    Path(source),
                    destination,
                    digest=digest,
                    length=length,
                    cancelled=cancellation.is_set,
                ),
            )
            try:
                await asyncio.shield(future)
            except asyncio.CancelledError as cancelled:
                cancellation.set()
                while not future.done():
                    try:
                        await asyncio.shield(future)
                    except asyncio.CancelledError:
                        continue
                    except Exception:
                        break
                if not future.cancelled() and (failure := future.exception()) is not None:
                    if not isinstance(failure, InterruptedError):
                        cancelled.add_note(f"Delivery cleanup failed: {failure}")
                raise
            except OSError as error:
                raise NousError(
                    "content.delivery_failed",
                    "Cannot deliver the saved content",
                    details={
                        "artifact": to_jsonable_python(ref, round_trip=True),
                        "destination": str(destination),
                    },
                    cause=error,
                ) from error

        budget = cast(float | None, remaining)
        asyncio.run(_client_work(deliver(), sock, budget))
        sock.sendall(encode({"kind": "export_done"}))
        _check_result(receive_sync(sock))
        return True
    except _HostResponse:
        _check_result(receive_sync(sock))
        raise NousError("instance.control_lost", "The local host ended the delivery") from None
    except TimeoutError:
        raise
    except (EOFError, OSError, ValueError) as error:
        raise NousError(
            "instance.control_lost", "The local host connection was lost", cause=error
        ) from error
    finally:
        sock.close()


def _check_result(response: dict[str, object]) -> dict[str, object]:
    if isinstance(response.get("error"), dict):
        error = cast(dict[str, object], response["error"])
        raise NousError(
            str(error["code"]),
            str(error["message"]),
            details=cast(dict[str, object], error.get("details", {})),
            retryable=bool(error.get("retryable", False)),
            operation=cast(str | None, error.get("operation")),
            retry_after=cast(float | None, error.get("retry_after")),
            operation_id=cast(str | None, error.get("operation_id")),
            trace_id=cast(str | None, error.get("trace_id")),
        )
    return response
