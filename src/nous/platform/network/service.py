"""Own scoped HTTPX pools and retire changed policies after their active leases finish."""

from __future__ import annotations

import asyncio
import ssl
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager
from dataclasses import dataclass
from typing import TYPE_CHECKING

import httpx2
from nous_sdk.contracts import CallContext

from nous.platform.concurrency import owned_thread
from nous.platform.errors import Unavailable

from .policy import NetworkPolicy

if TYPE_CHECKING:
    from nous.platform.configuration.service import ConfigurationService


@dataclass
class _Pool:
    client: httpx2.AsyncClient
    scope: tuple[str, str, str | None]
    policy: str
    users: int = 0
    retired: bool = False


class NetworkService:
    def __init__(self, configuration: ConfigurationService) -> None:
        self.configuration = configuration
        self._pools: dict[tuple[str, str, str | None, str, float | None], _Pool] = {}
        self._tls: dict[tuple[bool, str | None, bool], ssl.SSLContext] = {}
        self._condition = asyncio.Condition()
        self._closing = False
        self._shutdown_task: asyncio.Task[None] | None = None

    async def _ssl(self, policy: NetworkPolicy) -> ssl.SSLContext:
        key = (policy.verify_tls, str(policy.ca_file) if policy.ca_file else None, policy.trust_env)
        if key not in self._tls:

            def create() -> ssl.SSLContext:
                if policy.ca_file is not None:
                    return ssl.create_default_context(cafile=str(policy.ca_file))
                return httpx2.create_ssl_context(
                    verify=policy.verify_tls, trust_env=policy.trust_env
                )

            try:
                self._tls[key] = await owned_thread(create)
            except (OSError, ssl.SSLError) as error:
                raise Unavailable(
                    "Outbound TLS configuration is unavailable",
                    context={"component": "network", "stage": "tls"},
                ) from error
        return self._tls[key]

    @asynccontextmanager
    async def client(
        self, ctx: CallContext, *, timeout_seconds: float | None = None
    ) -> AsyncGenerator[httpx2.AsyncClient]:
        effective = await self.configuration.effective(ctx, "network")
        policy = NetworkPolicy.model_validate(effective["value"])
        fingerprint = policy.model_dump_json()
        scope = (ctx.caller_id, ctx.namespace_id, ctx.subject_id)
        key = (*scope, fingerprint, timeout_seconds)
        async with self._condition:
            if self._closing:
                raise RuntimeError("Network service is closing")
            for old_key, old in tuple(self._pools.items()):
                if old.scope == scope and old.policy != fingerprint:
                    old.retired = True
                    if old.users == 0:
                        await self._complete_cleanup(asyncio.create_task(old.client.aclose()))
                        del self._pools[old_key]
            pool = self._pools.get(key)
            if pool is None:
                tls = await self._ssl(policy)

                def timeout(value: float) -> float:
                    return min(value, timeout_seconds) if timeout_seconds is not None else value

                pool = _Pool(
                    httpx2.AsyncClient(
                        verify=tls,
                        trust_env=policy.trust_env,
                        proxy=policy.proxy_url,
                        follow_redirects=policy.follow_redirects,
                        timeout=httpx2.Timeout(
                            connect=timeout(policy.connect_timeout_seconds),
                            read=timeout(policy.read_timeout_seconds),
                            write=timeout(policy.write_timeout_seconds),
                            pool=timeout(policy.pool_timeout_seconds),
                        ),
                        limits=httpx2.Limits(
                            max_connections=policy.max_connections,
                            max_keepalive_connections=policy.max_keepalive_connections,
                            keepalive_expiry=policy.keepalive_expiry_seconds,
                        ),
                    ),
                    scope,
                    fingerprint,
                )
                self._pools[key] = pool
            pool.retired = False
            pool.users += 1
        try:
            yield pool.client
        finally:
            await self._complete_cleanup(asyncio.create_task(self._release(key, pool)))

    async def _release(
        self, key: tuple[str, str, str | None, str, float | None], pool: _Pool
    ) -> None:
        async with self._condition:
            pool.users -= 1
            try:
                if pool.users == 0 and pool.retired:
                    await pool.client.aclose()
                    self._pools.pop(key, None)
            finally:
                self._condition.notify_all()

    @staticmethod
    async def _complete_cleanup(work: asyncio.Task[None]) -> None:
        cancelled: asyncio.CancelledError | None = None
        while not work.done():
            try:
                await asyncio.shield(work)
            except asyncio.CancelledError as error:
                cancelled = error
            except BaseException:
                break
        try:
            work.result()
        except BaseException as error:
            if cancelled is not None:
                raise BaseExceptionGroup(
                    "Network cleanup failed during cancellation", [cancelled, error]
                ) from None
            raise
        if cancelled is not None:
            raise cancelled

    async def close(self) -> None:
        if self._shutdown_task is None:
            self._shutdown_task = asyncio.create_task(self._shutdown())
        await self._complete_cleanup(self._shutdown_task)

    async def _shutdown(self) -> None:
        async with self._condition:
            self._closing = True
            await self._condition.wait_for(
                lambda: all(pool.users == 0 for pool in self._pools.values())
            )
            clients = [pool.client for pool in self._pools.values()]
            self._pools.clear()
            self._tls.clear()
        results = await asyncio.gather(
            *(client.aclose() for client in clients), return_exceptions=True
        )
        failures = [result for result in results if isinstance(result, BaseException)]
        if failures:
            raise BaseExceptionGroup("Network client shutdown failed", failures)
