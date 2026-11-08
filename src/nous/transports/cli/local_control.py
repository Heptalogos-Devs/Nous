"""Serve local CLI invocations through the Runtime that owns the instance locks."""

from __future__ import annotations

import asyncio
import errno
import hmac
import json
import os
import secrets
import shutil
import socket
import sys
import tempfile
import threading
from collections.abc import AsyncGenerator, Callable, Mapping, Sequence
from contextlib import asynccontextmanager
from pathlib import Path
from typing import TYPE_CHECKING, cast

from nous.common.errors import NousError
from nous.content.input_access import current_input_access
from nous.runtime.instance import InstanceLocator

from .control_files import ClientFiles
from .control_wire import (
    MAX_FRAME,
    encode,
    endpoint,
    proof,
    read_endpoint,
    receive,
    receive_sync,
    send,
)

if TYPE_CHECKING:
    from nous.runtime.bootstrap import Runtime


class HostCall:
    def __init__(
        self,
        runtime: Runtime,
        locator: InstanceLocator,
        cwd: str,
        writer: asyncio.StreamWriter,
    ) -> None:
        self.runtime, self.locator, self.writer = runtime, locator, writer
        self.loop = asyncio.get_running_loop()
        self.cwd = Path(cwd)
        if not self.cwd.is_absolute():
            raise ValueError("Client working directory must be absolute")
        self.closed = False
        self.cancelled = False
        self.task: asyncio.Task[object] | None = None
        self.input_waiter: asyncio.Future[object] | None = None
        self.input_id: str | None = None
        self.input_lock = asyncio.Lock()
        self.staging: Path | None = None
        self.staged: dict[Path, Path] = {}
        self.staged_sources: dict[Path, Path] = {}
        self.resolved_sources: dict[Path, Path] = {}

    def original(self, path: Path) -> Path:
        original = self.staged.get(path, path)
        return original if original.is_absolute() else self.cwd / original

    async def _request_input(self, message: dict[str, object]) -> object:
        async with self.input_lock:
            if self.cancelled:
                raise asyncio.CancelledError
            self.input_waiter = self.loop.create_future()
            self.input_id = secrets.token_hex(16)
            try:
                await send(self.writer, {**message, "request_id": self.input_id})
                timeout = self.runtime.execution.snapshot.values.execution.default_timeout_seconds
                return await asyncio.wait_for(self.input_waiter, timeout)
            finally:
                self.input_waiter = None
                self.input_id = None

    async def read_stream(self) -> str:
        value = await self._request_input({"kind": "stdin"})
        if not isinstance(value, str):
            raise ValueError("Invalid client standard input")
        return value

    def _staging_file(self, name: str) -> Path:
        if self.staging is None:
            control_path = endpoint(self.locator, create=True)
            assert control_path is not None
            self.staging = Path(
                tempfile.mkdtemp(prefix=f".{control_path.stem}.input-", dir=control_path.parent)
            )
        target = self.staging / secrets.token_hex(12) / name
        target.parent.mkdir(mode=0o700)
        return target

    async def materialize(self, path: Path) -> Path:
        if self.closed or self.cancelled:
            raise NousError("instance.control_lost", "The client input session has ended")
        if path in self.staged:
            return path
        if not path.is_absolute():
            path = self.cwd / path
        if path in self.staged_sources:
            return self.staged_sources[path]
        target = self._staging_file(path.name or "input")
        resolved = await self._request_input(
            {"kind": "file", "source": str(path), "target": str(target)}
        )
        if not isinstance(resolved, str) or not Path(resolved).is_absolute():
            raise ValueError("Invalid resolved client input path")
        canonical = Path(resolved)
        self.staged[target] = path
        self.staged_sources[path] = target
        self.staged_sources[canonical] = target
        self.resolved_sources[path] = canonical
        return target

    async def read_text(self, path: Path, selection: object, limit: int) -> str:
        if self.closed or self.cancelled:
            raise NousError("instance.control_lost", "The client input session has ended")
        if not path.is_absolute():
            path = self.cwd / path
        target = self._staging_file("selection.txt")
        from nous.content.contracts import SourceSelection

        if not isinstance(selection, SourceSelection):
            raise ValueError("Invalid text selection")
        await self._request_input(
            {
                "kind": "selected_text",
                "source": str(path),
                "target": str(target),
                "selection": selection.model_dump(mode="json"),
                "limit": limit,
            }
        )
        return (await self.runtime.execution.blocking(target.read_bytes)).decode("utf-8")

    async def resolve(self, path: Path) -> Path:
        if not path.is_absolute():
            path = self.cwd / path
        if path in self.resolved_sources:
            return self.resolved_sources[path]
        value = await self._request_input({"kind": "resolve", "source": str(path)})
        if not isinstance(value, str) or not Path(value).is_absolute():
            raise ValueError("Invalid resolved client path")
        self.resolved_sources[path] = Path(value)
        return Path(value)

    async def write_text(self, target: Path, text: str) -> None:
        await self._request_input({"kind": "write_text", "target": str(target), "text": text})

    async def export_file(self, source: Path, target: Path, digest: str, length: int) -> None:
        key = secrets.token_hex(16)
        temporary = self._staging_file("export")
        await self._request_input(
            {
                "kind": "export_prepare",
                "key": key,
                "source": str(source),
                "target": str(target),
                "temporary": str(temporary),
                "digest": digest,
                "length": length,
            }
        )
        await self._request_input({"kind": "export_commit", "key": key})

    def event(self, name: str, value: object) -> None:
        from nous.application.actions import encode as encode_value

        if self.writer.transport.get_write_buffer_size() > MAX_FRAME:
            raise ValueError("Local control output backpressure limit exceeded")
        self.writer.write(encode({"kind": "event", "name": name, "value": encode_value(value)}))

    def cancel(self) -> None:
        self.cancelled = True
        if self.task is not None:
            self.task.cancel()
        if self.input_waiter is not None and not self.input_waiter.done():
            self.input_waiter.cancel()

    async def invoke_action(self, request: dict[str, object]) -> dict[str, object]:
        from pydantic import TypeAdapter

        from nous.application.actions import encode as encode_value
        from nous.application.actions import perform
        from nous.application.inputs import InputParameter
        from nous.common.errors import failure_code, failure_message
        from nous.common.operations import exception_context

        name, namespace = request.get("action"), request.get("namespace")
        args, kwargs, event_names = (
            request.get("args"),
            request.get("kwargs"),
            request.get("events"),
        )
        if (
            not isinstance(name, str)
            or (namespace is not None and not isinstance(namespace, str))
            or not isinstance(args, list)
            or not isinstance(kwargs, dict)
            or not isinstance(event_names, list)
        ):
            raise ValueError("Invalid application operation")
        if any(event not in {"on_delta", "on_event"} for event in cast(list[object], event_names)):
            raise ValueError("Invalid application operation event")
        token = current_input_access.set(self)
        self.task = cast(asyncio.Task[object], asyncio.current_task())
        try:
            events: dict[str, Callable[..., object]] = {}
            if "on_delta" in event_names:

                def on_delta(value: str) -> None:
                    self.event("delta", value)

                events["on_delta"] = on_delta
            if "on_event" in event_names:
                events["on_event"] = self.event
            if name == "configuration.write_schema":
                events["on_write"] = self.write_text
            if name == "tools.content_export":
                events["on_export"] = self.export_file
            value = await perform(
                self.runtime,
                name,
                namespace,
                cast(list[object], args),
                cast(dict[str, object], kwargs),
                events=events,
                inputs=TypeAdapter(tuple[InputParameter, ...]).validate_python(
                    request.get("inputs", [])
                ),
                read_stream=self.read_stream,
            )
            return {"kind": "result", "value": encode_value(value)}
        except Exception as error:
            failure = (
                error
                if isinstance(error, NousError)
                else NousError(
                    failure_code(error), failure_message(error), details=exception_context(error)
                )
            )
            return {"kind": "result", "error": failure.to_dict()}
        finally:
            self.task = None
            current_input_access.reset(token)


class LocalControl:
    def __init__(self, runtime: Runtime, locator: InstanceLocator) -> None:
        self.runtime, self.locator = runtime, locator
        self.server: asyncio.AbstractServer | None = None
        self.path: Path | None = None
        self.inode: int | None = None
        self.secret = secrets.token_hex(32)
        self.handlers: dict[asyncio.Task[None], tuple[asyncio.StreamWriter, HostCall | None]] = {}
        self.slots = asyncio.Semaphore(16)
        self.handshakes: dict[asyncio.Task[None], asyncio.StreamWriter] = {}
        self.authorized_waiters = 0

    async def start(self) -> None:
        path = endpoint(self.locator, create=True)
        if path is None:
            return
        read_endpoint(path)  # Refuse an unsafe stale endpoint; the instance locks are held.
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
        # An idle unauthenticated peer must never reserve a business slot. Evict
        # the oldest pending handshake to let a new legitimate peer make progress.
        if len(self.handshakes) >= 32:
            oldest = next(iter(self.handshakes))
            self.handshakes.pop(oldest).close()
            oldest.cancel()
        task = asyncio.create_task(self.handle(reader, writer))
        self.handshakes[task] = writer
        self.handlers[task] = (writer, None)

        def finished(done: asyncio.Task[None]) -> None:
            self.handshakes.pop(done, None)
            self.handlers.pop(done, None)

        task.add_done_callback(finished)

    async def handle(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        task = asyncio.current_task()
        assert task is not None
        call: HostCall | None = None
        worker: asyncio.Task[dict[str, object]] | None = None
        try:
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
                expected = proof(self.secret, "client", nonce, server_nonce)
                client_proof = auth.get("proof")
                if (
                    auth.get("kind") != "auth"
                    or not isinstance(client_proof, str)
                    or not hmac.compare_digest(client_proof, expected)
                ):
                    raise ValueError("Local control authentication failed")
            self.handshakes.pop(task, None)
            if self.authorized_waiters >= 64:
                raise ValueError("Too many waiting local control requests")
            self.authorized_waiters += 1
            try:
                await self.slots.acquire()
            finally:
                self.authorized_waiters -= 1
            try:
                request = await asyncio.wait_for(receive(reader), 600)
                args, cwd = request.get("args"), request.get("cwd")
                if not isinstance(args, list) or not isinstance(cwd, str):
                    raise ValueError("Invalid local application request")
                call = HostCall(self.runtime, self.locator, cwd, writer)
                self.handlers[task] = (writer, call)
                worker = asyncio.create_task(call.invoke_action(request))
                incoming = asyncio.create_task(receive(reader))
                try:
                    while True:
                        done, _ = await asyncio.wait(
                            (worker, incoming), return_when=asyncio.FIRST_COMPLETED
                        )
                        if worker in done:
                            incoming.cancel()
                            await asyncio.gather(incoming, return_exceptions=True)
                            await send(writer, worker.result())
                            break
                        message = incoming.result()
                        kind = message.get("kind")
                        if kind in {
                            "file_result",
                            "stdin_result",
                            "output_result",
                            "resolve_result",
                            "stat_result",
                        }:
                            if (
                                call.input_waiter is None
                                or message.get("request_id") != call.input_id
                            ):
                                # A timed-out client operation can finish later.
                                # Never deliver its reply to another I/O request.
                                incoming = asyncio.create_task(receive(reader))
                                continue
                            waiter = call.input_waiter
                            value = (
                                message.get("text")
                                if kind == "stdin_result"
                                else message.get("resolved")
                                if kind in {"file_result", "resolve_result"}
                                else message.get("value")
                                if kind == "stat_result"
                                else None
                            )
                        else:
                            raise ValueError("Unexpected local control message")
                        if waiter.done():
                            incoming = asyncio.create_task(receive(reader))
                            continue
                        if message.get("error") is not None:
                            code = message.get("code")
                            waiter.set_exception(
                                NousError(code, str(message["error"]))
                                if isinstance(code, str)
                                else OSError(str(message["error"]))
                            )
                        else:
                            waiter.set_result(value)
                        incoming = asyncio.create_task(receive(reader))
                finally:
                    if not incoming.done():
                        incoming.cancel()
                        await asyncio.gather(incoming, return_exceptions=True)
            finally:
                self.slots.release()
        except Exception, asyncio.CancelledError:
            if call is not None:
                call.cancel()
            if worker is not None:
                await asyncio.gather(worker, return_exceptions=True)
        finally:
            if call is not None:
                call.closed = True
                if call.staging is not None:
                    await asyncio.to_thread(shutil.rmtree, call.staging, ignore_errors=True)
            writer.close()
            try:
                await writer.wait_closed()
            except OSError:
                pass

    async def stop(self) -> None:
        if self.server is not None:
            self.server.close()
            await self.server.wait_closed()
        for writer, call in list(self.handlers.values()):
            if call is not None:
                call.cancel()
            writer.close()
        if self.handlers:
            await asyncio.gather(*list(self.handlers), return_exceptions=True)
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
        from nous.runtime.execution import complete_cleanup

        await complete_cleanup(control.stop())


def request_action(
    locator: InstanceLocator,
    action: str,
    namespace: str | None,
    args: Sequence[object],
    kwargs: Mapping[str, object],
    on_delta: Callable[[str], None] | None = None,
    *,
    inputs: Sequence[object] = (),
    on_event: Callable[[str, object], None] | None = None,
) -> dict[str, object] | None:
    from nous.application.actions import encode as encode_value

    response = _control_request(
        locator,
        {
            "action": action,
            "namespace": namespace,
            "args": encode_value(args),
            "kwargs": encode_value(kwargs),
            "events": (["on_delta"] if on_delta is not None else [])
            + (["on_event"] if on_event is not None else []),
            "inputs": encode_value(inputs),
            "cwd": os.getcwd(),
            "stdin_tty": sys.stdin.isatty(),
        },
        on_delta=on_delta,
        on_event=on_event,
    )
    if response is not None and isinstance(response.get("error"), dict):
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


def _control_request(
    locator: InstanceLocator,
    request: dict[str, object],
    *,
    on_delta: Callable[[str], None] | None = None,
    on_event: Callable[[str, object], None] | None = None,
) -> dict[str, object] | None:
    path = endpoint(locator)
    info = read_endpoint(path) if path is not None else None
    if info is None:
        return None
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    files = ClientFiles()
    session_done = threading.Event()
    sock.settimeout(10)
    try:
        try:
            sock.connect(("127.0.0.1", info["port"]))
        except OSError as error:
            if error.errno in {errno.ENOENT, errno.ECONNREFUSED}:
                return None
            raise
        nonce = secrets.token_hex(32)
        sock.sendall(encode({"kind": "hello", "nonce": nonce}))
        try:
            hello = receive_sync(sock)
            server_nonce, server_proof = hello.get("nonce"), hello.get("proof")
            if (
                hello.get("kind") != "hello"
                or not isinstance(server_nonce, str)
                or len(server_nonce) != 64
                or not isinstance(server_proof, str)
                or not hmac.compare_digest(server_proof, proof(info["secret"], "server", nonce))
            ):
                raise ValueError("Local host authentication failed")
        except (EOFError, OSError, ValueError) as error:
            raise NousError(
                "instance.control_unavailable",
                "Cannot authenticate the local instance host",
                cause=error,
            ) from error
        sock.sendall(
            encode({"kind": "auth", "proof": proof(info["secret"], "client", nonce, server_nonce)})
        )
        sock.settimeout(None)
        send_lock = threading.Lock()

        def send_from_thread(message: dict[str, object]) -> None:
            try:
                payload = encode(message)
            except NousError as error:
                # JSON escaping can exceed the frame limit even when the raw
                # stdin bytes fit. Report it instead of losing the worker reply.
                payload = encode(
                    {
                        "kind": message.get("kind"),
                        "request_id": message.get("request_id"),
                        "error": error.message,
                    }
                )
            try:
                with send_lock:
                    sock.sendall(payload)
            except OSError:
                pass

        def input_worker(message: dict[str, object]) -> None:
            kind = message.get("kind")
            try:
                if kind == "stdin":
                    text = sys.stdin.read(MAX_FRAME + 1)
                    if len(text.encode("utf-8")) > MAX_FRAME:
                        raise ValueError("CLI standard input exceeds the local control limit")
                    result: dict[str, object] = {"kind": "stdin_result", "text": text}
                elif kind == "resolve":
                    source = message.get("source")
                    if not isinstance(source, str):
                        raise ValueError("Invalid local path resolution request")
                    result = {"kind": "resolve_result", "resolved": str(Path(source).resolve())}
                elif kind == "selected_text":
                    source, target, limit = (
                        message.get("source"),
                        message.get("target"),
                        message.get("limit"),
                    )
                    if (
                        not isinstance(source, str)
                        or not isinstance(target, str)
                        or not isinstance(limit, int)
                    ):
                        raise ValueError("Invalid selected input request")
                    from nous.content.contracts import SourceSelection
                    from nous.storage.file_input import FileReaderPool

                    async def select_text() -> bytes:
                        selection = SourceSelection.model_validate(message.get("selection"))
                        pool = FileReaderPool(1)
                        chunks = pool.chunks(
                            Path(source), 1024 * 1024, selection=selection, limit=limit
                        )

                        async def consume() -> bytes:
                            return b"".join([chunk async for chunk in chunks])

                        task = asyncio.create_task(consume())
                        try:
                            while not task.done():
                                await asyncio.wait((task,), timeout=0.1)
                                if session_done.is_set():
                                    task.cancel()
                            return await task
                        finally:
                            await chunks.aclose()
                            await pool.aclose()

                    data = asyncio.run(select_text())
                    if session_done.is_set():
                        return
                    Path(target).write_bytes(data)
                    result = {"kind": "file_result", "resolved": source}
                elif kind in {"write_text", "export_prepare", "export_commit"}:
                    files.perform(message)
                    result = {"kind": "output_result"}
                else:
                    source, target = message.get("source"), message.get("target")
                    if not isinstance(source, str) or not isinstance(target, str):
                        raise ValueError("Invalid local input request")
                    resolved = Path(source).resolve()
                    with resolved.open("rb") as src, open(target, "xb") as dst:
                        shutil.copyfileobj(src, dst, length=1024 * 1024)
                    result = {"kind": "file_result", "resolved": str(resolved)}
            except Exception as error:
                result = {
                    "kind": "stdin_result"
                    if kind == "stdin"
                    else "output_result"
                    if kind in {"write_text", "export_prepare", "export_commit"}
                    else "resolve_result"
                    if kind == "resolve"
                    else "file_result",
                    "error": str(error),
                    "code": error.code if isinstance(error, NousError) else None,
                }
            result["request_id"] = message.get("request_id")
            send_from_thread(result)

        try:
            sock.sendall(encode(request))
            while True:
                response = receive_sync(sock)
                if response.get("kind") in {
                    "stdin",
                    "file",
                    "resolve",
                    "selected_text",
                    "write_text",
                    "export_prepare",
                    "export_commit",
                }:
                    threading.Thread(target=input_worker, args=(response,), daemon=True).start()
                    continue
                if response.get("kind") == "event":
                    name, value = response.get("name"), response.get("value")
                    if not isinstance(name, str):
                        raise ValueError("Invalid local event")
                    if name == "delta" and on_delta is not None:
                        if not isinstance(value, str):
                            raise ValueError("Invalid local delta")
                        on_delta(value)
                    elif on_event is not None:
                        on_event(name, value)
                    else:
                        raise ValueError("Unexpected local event")
                    continue
                if response.get("kind") != "result":
                    raise ValueError("Invalid local host response")
                return response
        except (EOFError, OSError, ValueError) as error:
            raise NousError(
                "instance.control_lost",
                "The local host connection was lost; the operation may have completed. Inspect its result before retrying.",
                cause=error,
            ) from error
    finally:
        session_done.set()
        sock.close()
        files.cleanup()
