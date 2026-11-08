"""Serve local CLI invocations through the Runtime that owns the instance locks."""

from __future__ import annotations

import asyncio
import concurrent.futures
import errno
import hmac
import json
import os
import secrets
import shutil
import socket
import stat
import sys
import tempfile
import threading
from collections.abc import AsyncGenerator, Awaitable, Sequence
from contextlib import asynccontextmanager
from pathlib import Path
from typing import TYPE_CHECKING, cast

import click
from typer.main import get_command

from nous.common.errors import NousError
from nous.content.input_access import current_input_access
from nous.runtime.instance import InstanceLocator
from nous.runtime.local_context import current_call

from .control_files import ClientFiles
from .control_streams import ClientInput, ForwardOutput, isolated_cli_streams
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
    import typer

    from nous.runtime.bootstrap import Runtime


def _allowed_command(command: Sequence[str]) -> bool:
    return (
        bool(command)
        and command[0] not in {"init", "instance", "job"}
        and command[:2] != ["runtime", "run"]
    )


class HostCall:
    def __init__(
        self,
        runtime: Runtime,
        locator: InstanceLocator,
        cwd: str,
        stdin: str | None,
        stdin_tty: bool,
        writer: asyncio.StreamWriter,
    ) -> None:
        self.runtime, self.locator, self.writer = runtime, locator, writer
        self.loop = asyncio.get_running_loop()
        self.cwd = Path(cwd)
        if not self.cwd.is_absolute():
            raise ValueError("Client working directory must be absolute")
        self.stdin = ClientInput(stdin, stdin_tty, self.read_stdin)
        self.stdout = ForwardOutput(lambda value: self.output("stdout", value))
        self.stderr = ForwardOutput(lambda value: self.output("stderr", value))
        self.closed = False
        self.cancelled = False
        self.task: asyncio.Task[object] | None = None
        self.editor_waiter: asyncio.Future[object] | None = None
        self.input_waiter: asyncio.Future[object] | None = None
        self.input_id: str | None = None
        self.input_lock = asyncio.Lock()
        self.staging: Path | None = None
        self.staged: dict[Path, Path] = {}
        self.staged_sources: dict[Path, Path] = {}
        self.resolved_sources: dict[Path, Path] = {}

    def original(self, path: Path) -> Path:
        return self.staged.get(path, path)

    def _require_worker(self) -> None:
        try:
            running = asyncio.get_running_loop()
        except RuntimeError:
            return
        if running is self.loop:
            raise RuntimeError("Synchronous client I/O cannot run on the host event loop")

    async def _request_input(self, message: dict[str, object]) -> object:
        async with self.input_lock:
            if self.cancelled:
                raise asyncio.CancelledError
            self.input_waiter = self.loop.create_future()
            self.input_id = secrets.token_hex(16)
            try:
                await send(self.writer, {**message, "request_id": self.input_id})
                timeout = (
                    self.runtime.configuration.snapshot.values.execution.default_timeout_seconds
                )
                return await asyncio.wait_for(self.input_waiter, timeout)
            finally:
                self.input_waiter = None
                self.input_id = None

    def read_stdin(self) -> str:
        self._require_worker()
        value = asyncio.run_coroutine_threadsafe(
            self._request_input({"kind": "stdin"}), self.loop
        ).result()
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

    def materialize_sync(self, path: Path) -> Path:
        self._require_worker()
        return asyncio.run_coroutine_threadsafe(self.materialize(path), self.loop).result()

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

    def resolve_sync(self, path: Path) -> Path:
        self._require_worker()
        return asyncio.run_coroutine_threadsafe(self.resolve(path), self.loop).result()

    def path_info_sync(self, path: Path, *, resolve: bool) -> dict[str, object]:
        self._require_worker()
        value = asyncio.run_coroutine_threadsafe(
            self._request_input({"kind": "stat", "source": str(path), "resolve": resolve}),
            self.loop,
        ).result()
        if not isinstance(value, dict):
            raise ValueError("Invalid client path information")
        return cast(dict[str, object], value)

    def write_text_sync(self, target: Path, text: str) -> None:
        self._require_worker()
        asyncio.run_coroutine_threadsafe(
            self._request_input({"kind": "write_text", "target": str(target), "text": text}),
            self.loop,
        ).result()

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

    def output(self, stream: str, value: str) -> None:
        """Write from either the host event loop or its CLI worker thread."""
        message = {"kind": "output", "stream": stream, "text": value}
        try:
            running = asyncio.get_running_loop()
        except RuntimeError:
            running = None
        if running is self.loop:
            if self.writer.transport.get_write_buffer_size() > MAX_FRAME:
                raise ValueError("Local control output backpressure limit exceeded")
            self.writer.write(encode(message))
        else:
            asyncio.run_coroutine_threadsafe(send(self.writer, message), self.loop).result()

    def run[T](self, action: Awaitable[T]) -> T:
        result: concurrent.futures.Future[T] = concurrent.futures.Future()

        async def execute() -> T:
            return await action

        def start() -> None:
            task = self.loop.create_task(execute())
            self.task = cast(asyncio.Task[object], task)
            if self.cancelled:
                task.cancel()

            def finish(done: asyncio.Task[T]) -> None:
                self.task = None
                try:
                    result.set_result(done.result())
                except BaseException as error:
                    result.set_exception(error)

            task.add_done_callback(finish)

        self.loop.call_soon_threadsafe(start)
        return result.result()

    def cancel(self) -> None:
        self.cancelled = True
        if self.task is not None:
            self.task.cancel()
        if self.editor_waiter is not None and not self.editor_waiter.done():
            self.editor_waiter.cancel()
        if self.input_waiter is not None and not self.input_waiter.done():
            self.input_waiter.cancel()

    def edit(self, filename: str) -> None:
        self._require_worker()

        async def request() -> None:
            if self.cancelled:
                raise asyncio.CancelledError
            self.editor_waiter = self.loop.create_future()
            await send(self.writer, {"kind": "editor", "filename": filename})
            try:
                await self.editor_waiter
            finally:
                self.editor_waiter = None

        asyncio.run_coroutine_threadsafe(request(), self.loop).result()

    def invoke(self, args: list[str]) -> dict[str, object]:
        from . import cli, main

        token = current_call.set(self)
        input_token = current_input_access.set(self)
        try:
            parsed = get_command(cli).make_context("nous", args.copy(), resilient_parsing=True)
            command = [*parsed._protected_args, *parsed.args]  # pyright: ignore[reportPrivateUsage]
            selected = InstanceLocator.resolve(
                parsed.params.get("instance"), profile=parsed.params.get("profile")
            )
            if selected.directory != self.locator.directory or not _allowed_command(command):
                raise ValueError("Local host request exceeds its instance or command boundary")
            try:
                main(args=args, forwarded=True)
                status = 0
            except SystemExit as error:
                status = (
                    error.code if isinstance(error.code, int) else 0 if error.code is None else 1
                )
            return {
                "kind": "result",
                "status": status,
            }
        finally:
            current_input_access.reset(input_token)
            current_call.reset(token)


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
                stdin, stdin_tty = request.get("stdin"), request.get("stdin_tty")
                if (
                    not isinstance(args, list)
                    or not isinstance(cwd, str)
                    or (stdin is not None and not isinstance(stdin, str))
                    or not isinstance(stdin_tty, bool)
                ):
                    raise ValueError("Invalid local CLI request")
                raw_args = cast(list[object], args)
                if not all(isinstance(a, str) for a in raw_args):
                    raise ValueError("Invalid local CLI arguments")
                arguments = [cast(str, a) for a in raw_args]
                call = HostCall(self.runtime, self.locator, cwd, stdin, stdin_tty, writer)
                self.handlers[task] = (writer, call)
                worker = asyncio.create_task(asyncio.to_thread(call.invoke, arguments))
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
                        if kind == "editor_result" and call.editor_waiter is not None:
                            waiter = call.editor_waiter
                            value = None
                        elif kind in {
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
                            waiter.set_exception(OSError(str(message["error"])))
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
    with isolated_cli_streams():
        await control.start()
        try:
            yield
        finally:
            from nous.runtime.execution import complete_cleanup

            await complete_cleanup(control.stop())


def try_forward(cli: typer.Typer, argv: list[str]) -> int | None:
    """Forward only after connecting; never retry an indeterminate request locally."""
    if "--help" in argv or current_call.get() is not None:
        return None
    try:
        context = get_command(cli).make_context("nous", argv.copy(), resilient_parsing=True)
        command = [*context._protected_args, *context.args]  # pyright: ignore[reportPrivateUsage]
        if not _allowed_command(command):
            return None
        locator = InstanceLocator.resolve(
            context.params.get("instance"), profile=context.params.get("profile")
        )
        path = endpoint(locator)
        info = read_endpoint(path) if path is not None else None
        if info is None:
            return None
    except click.ClickException, ValueError:
        return None
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    files = ClientFiles()
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
        prefix = ["--instance", str(locator.directory)]
        if context.params.get("json_mode"):
            prefix.append("--json")
        if language := context.params.get("language"):
            prefix.extend(("--language", language))
        if context.params.get("debug"):
            prefix.append("--debug")
        stdin_tty = sys.stdin.isatty()
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
                elif kind == "stat":
                    source, resolve = message.get("source"), message.get("resolve")
                    if not isinstance(source, str) or not isinstance(resolve, bool):
                        raise ValueError("Invalid local path inspection request")
                    source = os.path.realpath(source) if resolve else source
                    try:
                        info = os.stat(source)
                    except OSError:
                        value: dict[str, object] = {"path": source, "exists": False}
                    else:
                        value = {
                            "path": source,
                            "exists": True,
                            "regular": stat.S_ISREG(info.st_mode),
                            "directory": stat.S_ISDIR(info.st_mode),
                            "readable": os.access(source, os.R_OK),
                            "writable": os.access(source, os.W_OK),
                            "executable": os.access(source, os.X_OK),
                        }
                    result = {"kind": "stat_result", "value": value}
                elif kind == "resolve":
                    source = message.get("source")
                    if not isinstance(source, str):
                        raise ValueError("Invalid local path resolution request")
                    result = {"kind": "resolve_result", "resolved": str(Path(source).resolve())}
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
                    else "stat_result"
                    if kind == "stat"
                    else "file_result",
                    "error": str(error),
                }
            result["request_id"] = message.get("request_id")
            send_from_thread(result)

        request = {
            "args": [*prefix, *command],
            "cwd": os.getcwd(),
            "stdin_tty": stdin_tty,
        }
        try:
            sock.sendall(encode(request))
            while True:
                response = receive_sync(sock)
                if response.get("kind") in {
                    "stdin",
                    "file",
                    "resolve",
                    "stat",
                    "write_text",
                    "export_prepare",
                    "export_commit",
                }:
                    threading.Thread(target=input_worker, args=(response,), daemon=True).start()
                    continue
                if response.get("kind") == "editor":
                    filename = response.get("filename")
                    if not isinstance(filename, str):
                        raise ValueError("Invalid local editor request")
                    try:
                        click.edit(filename=filename)
                        answer: dict[str, object] = {"kind": "editor_result"}
                    except Exception as error:
                        answer = {"kind": "editor_result", "error": str(error)}
                    with send_lock:
                        sock.sendall(encode(answer))
                    continue
                if response.get("kind") == "output":
                    stream, value = response.get("stream"), response.get("text")
                    if stream not in ("stdout", "stderr") or not isinstance(value, str):
                        raise ValueError("Invalid local output")
                    destination = sys.stdout if stream == "stdout" else sys.stderr
                    destination.write(value)
                    destination.flush()
                    continue
                status = response.get("status")
                if response.get("kind") != "result" or not isinstance(status, int):
                    raise ValueError("Invalid local host response")
                return status
        except (EOFError, OSError, ValueError) as error:
            raise NousError(
                "instance.control_lost",
                "The local host connection was lost; the operation may have completed. Inspect its result before retrying.",
                cause=error,
            ) from error
    finally:
        sock.close()
        files.cleanup()
