"""Run an ASGI application inside the host's existing event loop."""

import asyncio
import socket
from collections.abc import AsyncGenerator, Generator
from contextlib import asynccontextmanager, contextmanager
from pathlib import Path
from typing import TYPE_CHECKING

import uvicorn

from nous.runtime.execution import complete_cleanup

from .shared import ServiceAddress, authorized, validate_listener

if TYPE_CHECKING:
    from starlette.types import ASGIApp, Receive, Scope, Send


class BearerAuth:
    def __init__(self, app: ASGIApp, token: str) -> None:
        if not token:
            raise ValueError("Transport authentication token must be nonempty")
        self.app, self.token = app, token

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] == "http":
            headers = dict(scope.get("headers", ()))
            raw = headers.get(b"authorization", b"")
            if not authorized(raw.decode("latin-1"), self.token):
                await send(
                    {
                        "type": "http.response.start",
                        "status": 401,
                        "headers": [(b"www-authenticate", b"Bearer")],
                    }
                )
                await send({"type": "http.response.body", "body": b"Unauthorized"})
                return
        await self.app(scope, receive, send)


class _EmbeddedServer(uvicorn.Server):
    @contextmanager
    def capture_signals(self) -> Generator[None]:
        """The outer host owns process signals and cancellation."""
        yield


@asynccontextmanager
async def serve_asgi(
    app: ASGIApp,
    *,
    host: str,
    port: int,
    shutdown_grace: int = 30,
    tls_certfile: str | Path | None = None,
    tls_keyfile: str | Path | None = None,
) -> AsyncGenerator[ServiceAddress]:
    validate_listener(host, tls_certfile, tls_keyfile)
    family = socket.AF_INET6 if ":" in host else socket.AF_INET
    sock = socket.socket(family, socket.SOCK_STREAM)
    try:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        sock.bind((host, port))
        sock.listen(socket.SOMAXCONN)
        sock.setblocking(False)
    except BaseException:
        sock.close()
        raise
    actual = sock.getsockname()
    config = uvicorn.Config(
        app,
        host=host,
        port=port,
        loop="asyncio",
        log_config=None,
        access_log=False,
        timeout_graceful_shutdown=shutdown_grace,
        ssl_certfile=str(tls_certfile) if tls_certfile else None,
        ssl_keyfile=str(tls_keyfile) if tls_keyfile else None,
    )
    server = _EmbeddedServer(config)
    task = asyncio.create_task(server.serve(sockets=[sock]), name="nous-asgi-server")
    try:
        while not server.started:
            if task.done():
                await task
                raise RuntimeError("ASGI server exited before accepting connections")
            await asyncio.sleep(0.01)
        yield ServiceAddress(host=host, port=int(actual[1]))
    finally:
        server.should_exit = True
        try:
            await complete_cleanup(task)
        finally:
            sock.close()
