"""Hold namespace admission through the complete authorized HTTP response."""

from contextlib import AsyncExitStack

from starlette.types import ASGIApp, Receive, Scope, Send


class NamespaceQuotaMiddleware:
    def __init__(self, app: ASGIApp) -> None:
        self.app = app

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http":
            await self.app(scope, receive, send)
            return
        async with AsyncExitStack() as stack:
            scope["nous.namespace_admission"] = (stack, set())
            await self.app(scope, receive, send)
