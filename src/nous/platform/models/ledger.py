"""Call intents, paid outputs and budgets belong to one authority."""

import hashlib
import json
from datetime import UTC, datetime
from uuid import uuid4

import sqlalchemy as sa
from nous_wave.contracts import CallContext
from pydantic import JsonValue, TypeAdapter

from nous.platform.errors import NousError
from nous.platform.persistence import Authority, WriteTx, metadata

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


class CallLedger:
    def __init__(self, authority: Authority) -> None:
        self.authority = authority

    async def purge_materials_in(self, tx: WriteTx, material_ids: tuple[str, ...]) -> None:
        selected = set(material_ids)
        rows = (
            await tx.connection.execute(
                sa.select(calls).where(calls.c.namespace_id == tx.ctx.namespace_id)
            )
        ).mappings()
        for row in rows:
            request = row["request"]
            source = request.get("prompt", {}).get("variables", {}).get("source", {})
            inputs = set(request.get("source_ids", []))
            if source.get("material_id") in selected or inputs.intersection(selected):
                await tx.connection.execute(
                    calls.update()
                    .where(calls.c.id == row["id"])
                    .values(request={"purged": True}, result=None, usage=None, status="purged")
                )

    async def reserve(
        self, ctx: CallContext, purpose: str, request: dict[str, object], maximum: int
    ) -> tuple[str, object | None]:
        digest = hashlib.sha256(json.dumps(request, sort_keys=True).encode()).hexdigest()
        day = datetime.now(UTC).date().isoformat()
        async with self.authority.write(
            ctx,
            str(uuid4()),
            operation_kind="model.reserve",
            payload={"request_hash": digest, "purpose": purpose},
        ) as tx:
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
            if cached:
                tx.result({"id": cached["id"], "reused": True})
                return str(cached["id"]), cached["result"]
            count = int(
                await tx.connection.scalar(
                    sa.select(sa.func.count())
                    .select_from(calls)
                    .where(*predicate, calls.c.day == day)
                )
                or 0
            )
            if count >= maximum:
                raise NousError(
                    "ResourceExhausted", f"Daily {purpose} call budget exhausted", status=429
                )
            call_id = str(uuid4())
            await tx.connection.execute(
                calls.insert().values(
                    id=call_id,
                    namespace_id=ctx.namespace_id,
                    subject_id=ctx.subject_id,
                    purpose=purpose,
                    request_hash=digest,
                    day=day,
                    status="reserved",
                    request=request,
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
            if ctx.allowed_source_ids is not None:
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
