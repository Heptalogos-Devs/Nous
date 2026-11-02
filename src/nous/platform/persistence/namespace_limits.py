"""Namespace request admission and transactionally measured logical user storage."""

from __future__ import annotations

from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager
from math import inf
from typing import TYPE_CHECKING

import anyio
import sqlalchemy as sa
from nous_sdk.contracts import CallContext
from nous_sdk.contracts.namespace_policy import NamespaceQuota

from nous.platform.errors import NousError

from .schema import metadata

if TYPE_CHECKING:
    from nous.platform.configuration.service import ConfigurationValues

    from .authority import Authority, ReadView, WriteTx


# These are authoritative user records, including retained revision history.
# Import working sets, paid invocations, operation receipts, jobs, indexes and
# shared derived representations are system/rebuildable space, not this quota.
USER_TABLES = frozenset(
    {
        "materials",
        "observations",
        "source_regions",
        "cognitive_revisions",
        "external_resources",
        "subject_bindings",
    }
)


class NamespaceLimits:
    def __init__(self, values: ConfigurationValues) -> None:
        self.values = values
        self._requests: dict[str, anyio.CapacityLimiter] = {}

    @asynccontextmanager
    async def request(self, ctx: CallContext, *, authority: Authority) -> AsyncGenerator[None]:
        async with authority.read(ctx) as view:
            quota = await self.quota_in(view, ctx)
        limiter = self._requests.setdefault(ctx.namespace_id, anyio.CapacityLimiter(inf))
        limiter.total_tokens = quota.max_concurrent_requests or inf
        borrower = object()
        try:
            limiter.acquire_on_behalf_of_nowait(borrower)
        except anyio.WouldBlock as error:
            raise NousError(
                "namespace_request_limit",
                "Namespace concurrent request quota is exhausted",
                status=429,
                context={
                    "namespace_id": ctx.namespace_id,
                    "max_concurrent_requests": quota.max_concurrent_requests,
                },
            ) from error
        try:
            yield
        finally:
            limiter.release_on_behalf_of(borrower)

    async def quota_in(self, view: ReadView | WriteTx, ctx: CallContext) -> NamespaceQuota:
        return await self.values.policy_in(view, ctx, "namespace.quota", NamespaceQuota)

    async def usage_in(self, view: ReadView | WriteTx, ctx: CallContext) -> dict[str, object]:
        totals: dict[str, int] = {}
        for name in USER_TABLES:
            table = metadata.tables.get(name)
            if table is None or "namespace_id" not in table.c:
                continue
            payload = table.c.record if "record" in table.c else sa.func.json_array(*table.c)
            statement = sa.select(
                sa.func.coalesce(sa.func.sum(sa.func.length(sa.cast(payload, sa.LargeBinary))), 0)
            ).where(table.c.namespace_id == ctx.namespace_id)
            totals[name] = int(await view.connection.scalar(statement) or 0)
        # Original bytes are charged once per distinct content object within a
        # namespace, including binary objects shared physically across namespaces.
        material = metadata.tables.get("materials")
        if material is not None:
            objects = (
                sa.select(
                    material.c.content_hash,
                    sa.func.max(sa.func.json_extract(material.c.record, "$.byte_len")).label(
                        "byte_len"
                    ),
                )
                .where(
                    material.c.namespace_id == ctx.namespace_id,
                    sa.func.coalesce(sa.func.json_extract(material.c.record, "$.purged"), 0) == 0,
                )
                .group_by(material.c.content_hash)
                .subquery()
            )
            totals["original_content"] = int(
                await view.connection.scalar(
                    sa.select(sa.func.coalesce(sa.func.sum(objects.c.byte_len), 0))
                )
                or 0
            )
        limiter = self._requests.get(ctx.namespace_id)
        return {
            "active_requests": limiter.borrowed_tokens if limiter else 0,
            "storage_bytes": sum(totals.values()),
            "storage_components": totals,
            "storage_measure": "logical_utf8_records_and_distinct_original_bytes",
            "excluded_space": [
                "shared_derivations",
                "paid_model_invocations",
                "operation_receipts",
                "jobs",
                "import_working_sets",
                "indexes",
                "sqlite_overhead",
            ],
        }

    async def enforce_in(self, tx: WriteTx, ctx: CallContext, before: int) -> None:
        quota = await self.quota_in(tx, ctx)
        if quota.max_storage_bytes is None:
            return
        usage = await self.usage_in(tx, ctx)
        after = int(str(usage["storage_bytes"]))
        if after > quota.max_storage_bytes and after > before:
            raise NousError(
                "namespace_storage_limit",
                "Namespace logical user storage quota is exhausted",
                status=429,
                context={
                    "namespace_id": ctx.namespace_id,
                    "max_storage_bytes": quota.max_storage_bytes,
                    "storage_bytes": after,
                    "storage_bytes_before": before,
                },
            )
