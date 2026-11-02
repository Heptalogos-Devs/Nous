"""Call intents, paid outputs and budgets belong to one authority."""

import hashlib
import json
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import UTC, datetime
from uuid import uuid4

import sqlalchemy as sa
from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.model_calls import ArchivedCall, InvocationBinding, InvocationRestore
from nous_sdk.contracts.model_roles import ModelCachePolicy
from nous_sdk.contracts.model_routing import ModelCallOutcome, ModelRouteAttempt
from pydantic import JsonValue, TypeAdapter

from nous.platform.errors import NousError
from nous.platform.persistence import Authority, ReadView, WriteTx, metadata

from . import budgets, history, invocation_transfer
from .invocations import (
    InvocationAttempt,
    InvocationResult,
    attempt_in,
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
    origin_call_id: str | None = None


class CallLedger:
    async def embedding_candidates_in(
        self, view: ReadView | WriteTx, ctx: CallContext, text: str | None
    ) -> tuple[str, ...]:
        statement = sa.select(calls.c.id).where(
            calls.c.namespace_id == ctx.namespace_id,
            calls.c.status != "purged",
            calls.c.purpose == "embedding",
        )
        if text is not None:
            items = sa.func.json_each(calls.c.request, "$.texts").table_valued("value")
            statement = statement.where(
                sa.select(items.c.value).where(items.c.value == text).exists()
            )
        from .history import imported_calls

        historical = sa.select(imported_calls.c.id).where(
            imported_calls.c.namespace_id == ctx.namespace_id,
            imported_calls.c.record["status"].as_string() != "purged",
            imported_calls.c.record["purpose"].as_string() == "embedding",
        )
        if text is not None:
            items = sa.func.json_each(imported_calls.c.record, "$.request.texts").table_valued(
                "value"
            )
            historical = historical.where(
                sa.select(items.c.value).where(items.c.value == text).exists()
            )
        return tuple((await view.connection.scalars(statement)).all()) + tuple(
            (await view.connection.scalars(historical)).all()
        )

    async def export_invocations_in(self, view: ReadView | WriteTx, ctx: CallContext):
        return await invocation_transfer.export_in(view, ctx)

    async def restore_invocations_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        bindings: tuple[InvocationBinding, ...],
        *,
        namespace_mapping: Mapping[str, str],
        caller_mapping: Mapping[str, str],
        call_mapping: Mapping[str, str],
    ) -> InvocationRestore:
        return await invocation_transfer.restore_in(
            tx,
            ctx,
            bindings,
            namespace_mapping=namespace_mapping,
            caller_mapping=caller_mapping,
            call_mapping=call_mapping,
        )

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

    async def attempt(
        self, ctx: CallContext, role_id: str, invocation_id: str
    ) -> InvocationAttempt | None:
        async with self.authority.read(ctx) as view:
            return await attempt_in(view, ctx, role_id, invocation_id, calls)

    async def owns_invocation_call(
        self, ctx: CallContext, role: str, intent: str, call_id: str
    ) -> bool:
        async with self.authority.read(ctx) as view:
            identities = tuple(
                await view.connection.scalars(
                    sa.select(invocations.c.invocation_id).where(
                        invocations.c.call_id == call_id,
                        invocations.c.namespace_id == ctx.namespace_id,
                        invocations.c.caller_id == ctx.caller_id,
                        invocations.c.role == role,
                    )
                )
            )
            if not identities:
                return False
            if intent in identities:
                return True
            raw = await view.connection.scalar(
                sa.select(calls.c.request).where(
                    calls.c.id == call_id, calls.c.namespace_id == ctx.namespace_id
                )
            )
            if raw is None:
                historical = await view.connection.scalar(
                    sa.select(history.imported_calls.c.record).where(
                        history.imported_calls.c.id == call_id,
                        history.imported_calls.c.namespace_id == ctx.namespace_id,
                    )
                )
                if historical is None:
                    return False
                raw = ArchivedCall.model_validate(historical).request
            routing = raw.get("routing")
            return (
                isinstance(routing, dict)
                and ModelRouteAttempt.model_validate(routing).intent_id == intent
            )

    async def bind_success(
        self, ctx: CallContext, role_id: str, identity: str, call_id: str
    ) -> None:
        async with self.authority.write(
            ctx,
            identity,
            operation_kind="model.bind_success",
            scope=role_id,
            payload={"call_id": call_id},
        ) as tx:
            if tx.replayed:
                return
            await self.successful_in(tx, ctx, call_id)
            existing = await attempt_in(tx, ctx, role_id, identity, calls)
            if existing is None:
                await bind_in(tx, ctx, role_id, identity, call_id)
            elif existing.call_id != call_id:
                raise DomainError(
                    "model_invocation_conflict",
                    "The parent intent is already bound to another physical call",
                )
            tx.result({"call_id": call_id})

    async def purge_calls_in(self, tx: WriteTx, call_ids: tuple[str, ...]) -> None:
        if call_ids:
            await history.purge_in(tx, call_ids=call_ids)
            await tx.connection.execute(
                calls.update()
                .where(
                    calls.c.id.in_(call_ids),
                    calls.c.namespace_id == tx.ctx.namespace_id,
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
        if (ctx.namespace_id, ctx.namespace_id) != (tx.ctx.namespace_id, tx.ctx.namespace_id):
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
                        calls.c.status == "succeeded",
                    )
                )
            )
            .mappings()
            .first()
        )
        if row is None:
            historical = await view.connection.scalar(
                sa.select(history.imported_calls.c.record).where(
                    history.imported_calls.c.id == call_id,
                    history.imported_calls.c.namespace_id == ctx.namespace_id,
                )
            )
            if historical is not None:
                archived = ArchivedCall.model_validate(historical)
                if archived.status == "succeeded":
                    history.require_visible(ctx, archived.source_ids)
                    return PaidInvocation(
                        archived.id,
                        archived.request_hash,
                        archived.request,
                        archived.result,
                        archived.origin.call_id,
                    )
            raise DomainError(
                "model_call_unavailable", "A retained successful model call is required"
            )
        history.require_visible(
            ctx,
            tuple(
                sorted(
                    source_identities(
                        TypeAdapter(dict[str, JsonValue]).validate_python(row["request"])
                    )
                )
            ),
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

        def accepted_plan(value: JsonValue) -> bool:
            return (
                isinstance(value, dict)
                and "routes" in value
                and "prompt" in value
                and "role" in value
                and bool(source_identities(value).intersection(selected | purged_roots))
            )

        await self.authority.redact_receipts_in(tx, accepted_plan)
        await history.purge_in(tx, source_ids=selected | purged_roots)
        rows = (
            await tx.connection.execute(
                sa.select(calls).where(calls.c.namespace_id == tx.ctx.namespace_id)
            )
        ).mappings()
        for row in rows:
            request = row["request"]
            if not isinstance(request, dict):
                continue
            # Embedding and other non-generation calls store flat request fields without a prompt.
            prompt = request.get("prompt")
            variables = prompt.get("input_bindings") if isinstance(prompt, dict) else None
            source = variables.get("source") if isinstance(variables, dict) else None
            if not isinstance(source, dict):
                source = {}
            actual_inputs = source_identities(request)
            source_material_ids = source.get("source_material_ids")
            if isinstance(source_material_ids, list):
                actual_inputs.update(item for item in source_material_ids if isinstance(item, str))
            observation_ids = request.get("observation_ids")
            if (
                source.get("material_id") in selected
                or actual_inputs.intersection(selected)
                or actual_inputs.intersection(purged_roots)
                or source.get("observation_id") in purged_roots
                or (
                    isinstance(observation_ids, list)
                    and set(item for item in observation_ids if isinstance(item, str)).intersection(
                        purged_roots
                    )
                )
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
        budget_resource: str,
        resource_maximum: int,
        budget_role: str | None = None,
        invocation_id: str | None = None,
        cache_policy: ModelCachePolicy = "shared",
        routing: ModelRouteAttempt | None = None,
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
            payload={
                "request_hash": digest,
                "purpose": purpose,
                "role": role,
                "invocation_id": invocation_id,
            },
        ) as tx:
            if await erased_sources_in(tx, ctx, request):
                raise NousError(
                    "SourcePurged",
                    "Model input was permanently erased before admission",
                    status=409,
                )
            if invocation_id is not None:
                attempt = await attempt_in(tx, ctx, role, invocation_id, calls)
                if attempt is not None and attempt.request_hash != digest:
                    raise DomainError(
                        "model_invocation_conflict",
                        "Invocation identity already has a different provider input",
                    )
                if (
                    attempt is not None
                    and attempt.status == "failed"
                    and attempt.outcome is not None
                    and attempt.outcome.kind == "not_sent"
                ):
                    from nous.identity.access import require_namespace_in

                    await require_namespace_in(tx, ctx)
                    local = await tx.connection.scalar(
                        sa.select(calls.c.id).where(
                            calls.c.id == attempt.call_id, calls.c.namespace_id == ctx.namespace_id
                        )
                    )
                    if local is None:
                        raise DomainError(
                            "model_invocation_unavailable",
                            "An imported unfinished invocation remains frozen",
                            status="frozen",
                        )
                    # No physical request left this reservation. A later explicit
                    # execution may finish it, while reserved/unknown calls remain
                    # unavailable. The receipt retains the prior transport fact.
                    await budgets.reserve_in(
                        tx,
                        attempt.call_id,
                        role,
                        budget_resource,
                        day,
                        1,
                        maximum,
                        resource_maximum,
                    )
                    await tx.connection.execute(
                        calls.update()
                        .where(calls.c.id == attempt.call_id, calls.c.status == "failed")
                        .values(status="reserved", error_code=None, usage=None, day=day)
                    )
                    tx.result(
                        {
                            "id": attempt.call_id,
                            "reopened_not_sent": True,
                            "previous_error": attempt.error_code,
                            "previous_outcome": attempt.outcome,
                        }
                    )
                    return attempt.call_id, None
                retained = await invocation_in(tx, ctx, role, invocation_id, calls)
                if retained is not None:
                    tx.result({"id": retained.call_id, "reused": True})
                    return retained.call_id, retained.output
            predicate = (
                calls.c.namespace_id == ctx.namespace_id,
                calls.c.namespace_id == ctx.namespace_id,
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
            from nous.identity.access import require_namespace_in

            await require_namespace_in(tx, ctx)
            call_id = str(uuid4())
            await budgets.reserve_in(
                tx, call_id, role, budget_resource, day, 1, maximum, resource_maximum
            )
            if invocation_id is not None:
                await bind_in(tx, ctx, role, invocation_id, call_id)
            await tx.connection.execute(
                calls.insert().values(
                    id=call_id,
                    namespace_id=ctx.namespace_id,
                    purpose=purpose,
                    request_hash=digest,
                    day=day,
                    status="reserved",
                    request={
                        **request,
                        "budget_role": role,
                        "cache_policy": cache_policy,
                        **({"routing": routing.model_dump(mode="json")} if routing else {}),
                    },
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
        *,
        outcome: ModelCallOutcome | None = None,
    ) -> None:
        if error_code is not None:
            usage = {
                **usage,
                "outcome": (outcome or ModelCallOutcome(kind="unknown")).model_dump(mode="json"),
            }
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
            if state == "succeeded":
                tx.result({"id": call_id, "retained": True})
                return
            if state != "reserved":
                raise DomainError(
                    "model_call_terminal", "The physical call is already terminal", call_id=call_id
                )
            if error_code is None:
                actual_requests = TypeAdapter(int).validate_python(usage.get("requests", 1))
                await budgets.settle_in(tx, call_id, actual_requests)
            elif outcome is not None and outcome.kind == "not_sent":
                await budgets.settle_in(tx, call_id, 0)
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
