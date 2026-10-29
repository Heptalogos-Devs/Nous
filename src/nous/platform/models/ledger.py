"""Call intents, paid outputs and budgets belong to one authority."""

import hashlib
import json
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import UTC, datetime
from uuid import uuid4

import sqlalchemy as sa
from nous_wave.contracts import CallContext, DomainError
from nous_wave.contracts.model_calls import ArchivedCall
from nous_wave.contracts.model_roles import ModelCachePolicy
from pydantic import JsonValue, TypeAdapter

from nous.platform.errors import NousError
from nous.platform.persistence import Authority, ReadView, WriteTx, metadata

from . import history
from .invocations import (
    InvocationResult,
    bind_in,
    invocation_in,
    invocation_key,
    invocations,
    retire_in,
)
from .source_fence import erased_sources_in, source_identities

calls = sa.Table(
    "nous_model_calls",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("namespace_id", sa.String, nullable=False),
    sa.Column("subject_id", sa.String),
    sa.Column("purpose", sa.String, nullable=False),
    sa.Column("request_hash", sa.String, nullable=False, index=True),
    sa.Column("day", sa.String, nullable=False),
    sa.Column("status", sa.String, nullable=False),
    sa.Column("request", sa.JSON, nullable=False),
    sa.Column("result", sa.JSON),
    sa.Column("usage", sa.JSON),
    sa.Column("error_code", sa.String),
)


@dataclass(frozen=True)
class PaidInvocation:
    id: str
    request_hash: str
    request: dict[str, JsonValue]
    result: JsonValue


class CallLedger:
    def __init__(self, authority: Authority) -> None:
        self.authority = authority

    async def export_in(
        self, view: ReadView | WriteTx, ctx: CallContext, call_ids: tuple[str, ...]
    ) -> tuple[ArchivedCall, ...]:
        return await history.export_in(view, ctx, call_ids, calls)

    async def import_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        records: tuple[ArchivedCall, ...],
        mapping: Mapping[str, str],
        installation_id: str,
    ) -> dict[str, str]:
        return await history.import_in(tx, ctx, records, mapping, installation_id)

    async def recall(
        self, ctx: CallContext, role_id: str, invocation_id: str
    ) -> InvocationResult | None:
        async with self.authority.read(ctx) as view:
            return await invocation_in(view, ctx, role_id, invocation_id, calls)

    async def purge_calls_in(self, tx: WriteTx, call_ids: tuple[str, ...]) -> None:
        if call_ids:
            await history.purge_in(tx, call_ids=call_ids)
            await tx.connection.execute(
                calls.update()
                .where(
                    calls.c.id.in_(call_ids),
                    calls.c.namespace_id == tx.ctx.namespace_id,
                    calls.c.subject_id == tx.ctx.require_subject(),
                )
                .values(request={"purged": True}, result=None, usage=None, status="purged")
            )

    async def purge_invocations_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        role_id: str,
        identities: tuple[str, ...],
    ) -> None:
        if (ctx.namespace_id, ctx.subject_id) != (tx.ctx.namespace_id, tx.ctx.subject_id):
            raise ValueError("Invocation erasure must stay in the source transaction's subject")
        for identity in identities:
            await retire_in(tx, ctx, role_id, identity)
        ids = tuple(
            str(value)
            for value in (
                await tx.connection.execute(
                    sa.select(invocations.c.call_id).where(
                        invocations.c.key.in_(
                            tuple(invocation_key(ctx, role_id, identity) for identity in identities)
                        )
                    )
                )
            ).scalars()
        )
        await self.purge_calls_in(tx, ids)

    async def successful_in(
        self, view: ReadView | WriteTx, ctx: CallContext, call_id: str
    ) -> PaidInvocation:
        row = (
            (
                await view.connection.execute(
                    sa.select(calls).where(
                        calls.c.id == call_id,
                        calls.c.namespace_id == ctx.namespace_id,
                        calls.c.subject_id == ctx.require_subject(),
                        calls.c.status == "succeeded",
                    )
                )
            )
            .mappings()
            .first()
        )
        if row is None:
            raise DomainError(
                "model_call_unavailable", "A retained successful model call is required"
            )
        return PaidInvocation(
            str(row["id"]),
            str(row["request_hash"]),
            TypeAdapter(dict[str, JsonValue]).validate_python(row["request"]),
            TypeAdapter(JsonValue).validate_python(row["result"]),
        )

    async def purge_materials_in(self, tx: WriteTx, material_ids: tuple[str, ...]) -> None:
        from nous.material.service import observations

        purged_roots = {
            str(row["id"])
            for row in (
                await tx.connection.execute(
                    sa.select(observations.c.id, observations.c.record).where(
                        observations.c.namespace_id == tx.ctx.namespace_id,
                    )
                )
            ).mappings()
            if row["record"].get("purged")
        }
        selected = set(material_ids)
        await history.purge_in(tx, source_ids=selected | purged_roots)
        rows = (
            await tx.connection.execute(
                sa.select(calls).where(calls.c.namespace_id == tx.ctx.namespace_id)
            )
        ).mappings()
        for row in rows:
            request = row["request"]
            source = request.get("prompt", {}).get("variables", {}).get("source", {})
            inputs = source_identities(request)
            inputs.update(source.get("source_material_ids", []))
            if (
                source.get("material_id") in selected
                or inputs.intersection(selected)
                or inputs.intersection(purged_roots)
                or source.get("observation_id") in purged_roots
                or set(request.get("observation_ids", [])).intersection(purged_roots)
            ):
                await tx.connection.execute(
                    calls.update()
                    .where(calls.c.id == row["id"])
                    .values(request={"purged": True}, result=None, usage=None, status="purged")
                )

    async def reserve(
        self,
        ctx: CallContext,
        purpose: str,
        request: Mapping[str, object],
        maximum: int,
        *,
        budget_role: str | None = None,
        invocation_id: str | None = None,
        cache_policy: ModelCachePolicy = "shared",
    ) -> tuple[str, object | None]:
        request = dict(request)
        if cache_policy == "invocation" and invocation_id is None:
            raise DomainError(
                "model_invocation_required",
                "An invocation-scoped request requires its explicit invocation identity",
            )
        role = budget_role or purpose
        digest = hashlib.sha256(json.dumps(request, sort_keys=True).encode()).hexdigest()
        day = datetime.now(UTC).date().isoformat()
        async with self.authority.write(
            ctx,
            str(uuid4()),
            operation_kind="model.reserve",
            payload={"request_hash": digest, "purpose": purpose},
        ) as tx:
            if await erased_sources_in(tx, ctx, request):
                raise NousError(
                    "SourcePurged",
                    "Model input was permanently erased before admission",
                    status=409,
                )
            if invocation_id is not None:
                retained = await invocation_in(tx, ctx, role, invocation_id, calls)
                if retained is not None:
                    tx.result({"id": retained.call_id, "reused": True})
                    return retained.call_id, retained.output
            predicate = (
                calls.c.namespace_id == ctx.namespace_id,
                calls.c.subject_id == ctx.subject_id,
                calls.c.purpose == purpose,
            )
            cached = (
                (
                    await tx.connection.execute(
                        sa.select(calls).where(
                            *predicate,
                            calls.c.request_hash == digest,
                            calls.c.status == "succeeded",
                        )
                    )
                )
                .mappings()
                .first()
            )
            if cached and cache_policy == "shared":
                if invocation_id is not None:
                    await bind_in(tx, ctx, role, invocation_id, str(cached["id"]))
                tx.result({"id": cached["id"], "reused": True})
                return str(cached["id"]), cached["result"]
            count = int(
                await tx.connection.scalar(
                    sa.select(sa.func.count())
                    .select_from(calls)
                    .where(
                        calls.c.namespace_id == ctx.namespace_id,
                        calls.c.subject_id == ctx.subject_id,
                        calls.c.day == day,
                        sa.or_(
                            calls.c.request["budget_role"].as_string() == role,
                            sa.and_(
                                calls.c.request["budget_role"].as_string().is_(None),
                                calls.c.purpose.in_((role, f"media.{role}")),
                            ),
                        ),
                    )
                )
                or 0
            )
            if count >= maximum:
                raise NousError(
                    "ResourceExhausted", f"Daily {purpose} call budget exhausted", status=429
                )
            call_id = str(uuid4())
            if invocation_id is not None:
                await bind_in(tx, ctx, role, invocation_id, call_id)
            await tx.connection.execute(
                calls.insert().values(
                    id=call_id,
                    namespace_id=ctx.namespace_id,
                    subject_id=ctx.subject_id,
                    purpose=purpose,
                    request_hash=digest,
                    day=day,
                    status="reserved",
                    request={**request, "budget_role": role, "cache_policy": cache_policy},
                )
            )
            tx.result({"id": call_id, "reused": False})
            return call_id, None

    async def finish(
        self,
        ctx: CallContext,
        call_id: str,
        result: object,
        usage: dict[str, object],
        error_code: str | None = None,
    ) -> None:
        async with self.authority.write(
            ctx,
            str(uuid4()),
            operation_kind="model.finish",
            payload={"id": call_id, "error_code": error_code},
        ) as tx:
            state = await tx.connection.scalar(
                sa.select(calls.c.status).where(
                    calls.c.id == call_id, calls.c.namespace_id == ctx.namespace_id
                )
            )
            if state is None:
                raise LookupError("Model invocation does not belong to this namespace")
            if state == "purged":
                raise NousError(
                    "SourcePurged", "Model input was purged before completion", status=409
                )
            await tx.connection.execute(
                calls.update()
                .where(calls.c.id == call_id)
                .values(
                    status="failed" if error_code else "succeeded",
                    result=result,
                    usage=usage,
                    error_code=error_code,
                )
            )
            tx.result({"id": call_id})

    async def list(self, ctx: CallContext) -> list[dict[str, object]]:
        async with self.authority.read(ctx) as view:
            rows = (
                await view.connection.execute(
                    sa.select(calls).where(
                        calls.c.namespace_id == ctx.namespace_id,
                        calls.c.subject_id == ctx.subject_id,
                    )
                )
            ).mappings()
            result = [dict(row) for row in rows]
            result.extend(await history.list_in(view, ctx))
            if ctx.source_groups():
                for item in result:
                    item.pop("request", None)
                    item.pop("result", None)
                    usage = item.get("usage")
                    item["usage"] = (
                        {
                            key: value
                            for key, value in TypeAdapter(dict[str, JsonValue])
                            .validate_python(usage)
                            .items()
                            if key in {"input_tokens", "output_tokens", "total_tokens"}
                        }
                        if isinstance(usage, dict)
                        else None
                    )
            return result
