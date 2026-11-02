"""One durable caller invocation binds one actual paid request, including unknown outcomes."""

from dataclasses import dataclass
from hashlib import sha256

import sqlalchemy as sa
from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.model_routing import ModelCallOutcome
from pydantic import JsonValue, TypeAdapter
from sqlalchemy.dialects.sqlite import insert

from nous.platform.persistence import ReadView, WriteTx, metadata
from nous.platform.persistence.authority import canonical_json

invocations = sa.Table(
    "nous_model_invocations",
    metadata,
    sa.Column("key", sa.String, primary_key=True),
    sa.Column("call_id", sa.String, nullable=False),
    sa.Column("commit_seq", sa.Integer, nullable=False),
    *(
        sa.Column(name, sa.String, nullable=False)
        for name in ("namespace_id", "caller_id", "role", "invocation_id")
    ),
)

retirements = sa.Table(
    "nous_model_invocation_retirements",
    metadata,
    sa.Column("key", sa.String, primary_key=True),
    sa.Column("commit_seq", sa.Integer, nullable=False),
    sa.Column("recorded_at", sa.String, nullable=False),
    *(
        sa.Column(name, sa.String, nullable=False)
        for name in ("namespace_id", "caller_id", "role", "invocation_id", "reason")
    ),
)


async def retire_in(
    tx: WriteTx, ctx: CallContext, role_id: str, identity: str, *, reason: str = "source_purged"
) -> None:
    await tx.connection.execute(
        insert(retirements)
        .values(
            key=invocation_key(ctx, role_id, identity),
            commit_seq=tx.commit_seq,
            recorded_at=tx.recorded_at,
            namespace_id=ctx.namespace_id,
            caller_id=ctx.caller_id,
            role=role_id,
            invocation_id=identity,
            reason=reason,
        )
        .on_conflict_do_nothing(index_elements=["key"])
    )


def invocation_key(ctx: CallContext, role_id: str, identity: str) -> str:
    if not identity or len(identity) > 512:
        raise ValueError("Invocation identity requires 1–512 characters")
    return sha256(
        canonical_json((ctx.caller_id, ctx.namespace_id, role_id, identity)).encode()
    ).hexdigest()


def child(identity: str, index: int) -> str:
    return f"route:{sha256(identity.encode()).hexdigest()}:{index}"


@dataclass(frozen=True)
class InvocationResult:
    call_id: str
    output: JsonValue


@dataclass(frozen=True)
class InvocationAttempt:
    call_id: str
    status: str
    output: JsonValue
    error_code: str | None
    outcome: ModelCallOutcome | None
    request_hash: str
    request: dict[str, JsonValue]


async def attempt_in(
    view: ReadView | WriteTx,
    ctx: CallContext,
    role_id: str,
    identity: str,
    calls: sa.Table,
) -> InvocationAttempt | None:
    retired = (
        await view.connection.execute(
            sa.select(retirements.c.reason).where(
                retirements.c.key == invocation_key(ctx, role_id, identity)
            )
        )
    ).first()
    if retired is not None:
        raise DomainError(
            "model_invocation_unavailable",
            "The invocation is frozen after subject migration"
            if retired.reason in {"subject_migrated", "opaque_restored"}
            else "The invocation's input was permanently erased",
            invocation_id=identity,
            status="frozen"
            if retired.reason in {"subject_migrated", "opaque_restored"}
            else "purged",
        )
    row = (
        (
            await view.connection.execute(
                sa.select(
                    calls.c.id,
                    calls.c.status,
                    calls.c.result,
                    calls.c.error_code,
                    calls.c.usage,
                    calls.c.request_hash,
                    calls.c.request,
                )
                .join(invocations, invocations.c.call_id == calls.c.id)
                .where(
                    invocations.c.key == invocation_key(ctx, role_id, identity),
                    calls.c.namespace_id == ctx.namespace_id,
                )
            )
        )
        .mappings()
        .first()
    )
    if row is None:
        from nous_sdk.contracts.model_calls import ArchivedCall

        from .history import imported_calls, require_visible

        historical = await view.connection.scalar(
            sa.select(imported_calls.c.record)
            .join(invocations, invocations.c.call_id == imported_calls.c.id)
            .where(
                invocations.c.key == invocation_key(ctx, role_id, identity),
                imported_calls.c.namespace_id == ctx.namespace_id,
            )
        )
        if historical is None:
            return None
        archived = ArchivedCall.model_validate(historical)
        require_visible(ctx, archived.source_ids)
        usage = archived.usage or {}
        return InvocationAttempt(
            archived.id,
            archived.status,
            archived.result,
            archived.error_code,
            ModelCallOutcome.model_validate(usage["outcome"])
            if usage.get("outcome") is not None
            else None,
            archived.request_hash,
            archived.request,
        )
    from .history import require_visible
    from .source_fence import source_identities

    require_visible(
        ctx,
        tuple(
            sorted(
                source_identities(TypeAdapter(dict[str, JsonValue]).validate_python(row["request"]))
            )
        ),
    )
    usage = TypeAdapter(dict[str, JsonValue] | None).validate_python(row["usage"])
    outcome = (
        ModelCallOutcome.model_validate(usage["outcome"])
        if isinstance(usage, dict) and usage.get("outcome") is not None
        else None
    )
    return InvocationAttempt(
        str(row["id"]),
        str(row["status"]),
        TypeAdapter(JsonValue).validate_python(row["result"]),
        str(row["error_code"]) if row["error_code"] is not None else None,
        outcome,
        str(row["request_hash"]),
        TypeAdapter(dict[str, JsonValue]).validate_python(row["request"]),
    )


async def invocation_in(
    view: ReadView | WriteTx,
    ctx: CallContext,
    role_id: str,
    identity: str,
    calls: sa.Table,
) -> InvocationResult | None:
    row = await attempt_in(view, ctx, role_id, identity, calls)
    if row is None:
        return None
    status = row.status
    if status != "succeeded":
        raise DomainError(
            "model_invocation_unavailable",
            "The bound invocation has no retained successful result; a new invocation is required for another paid request",
            invocation_id=identity,
            call_id=row.call_id,
            status=status,
            error_code=row.error_code,
        )
    return InvocationResult(row.call_id, row.output)


async def bind_in(tx: WriteTx, ctx: CallContext, role_id: str, identity: str, call_id: str) -> None:
    await tx.connection.execute(
        invocations.insert().values(
            key=invocation_key(ctx, role_id, identity),
            call_id=call_id,
            commit_seq=tx.commit_seq,
            namespace_id=ctx.namespace_id,
            caller_id=ctx.caller_id,
            role=role_id,
            invocation_id=identity,
        )
    )
