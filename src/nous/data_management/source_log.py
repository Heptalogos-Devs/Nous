"""Read source operations by their actual log identity or committed sequence."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, DomainError, SourceResult
from pydantic import JsonValue
from sqlalchemy import select

from nous.identity.access import require_namespace_in
from nous.material.service import require_observation_in
from nous.platform.persistence import ReadView, WriteTx

from .schema import source_operations

if TYPE_CHECKING:
    from .sources import SourceService


async def authorize_result_in(
    self: SourceService, view: ReadView | WriteTx, ctx: CallContext, result: SourceResult
) -> None:
    await require_namespace_in(view, ctx)
    for observation_id in result.observation_ids:
        await require_observation_in(view, ctx, observation_id, active=False)
    roots = await self.cognition.source_roots_in(
        view, ctx, purpose="management", include_purged=True
    )
    for reference in result.affected or result.references:
        record = await self.cognition.get_in(view, ctx, reference, check_visibility=False)
        if not await self.cognition.visible_in(view, ctx, record, roots, purpose="management"):
            raise DomainError(
                "source_unavailable", "Operation objects exceed the current source scope"
            )


async def get_operation(
    self: SourceService,
    ctx: CallContext,
    source_operation_id: str | None = None,
    *,
    commit_seq: int | None = None,
):
    if (source_operation_id is None) == (commit_seq is None):
        raise DomainError(
            "source_operation_selector", "Select one source operation identity or commit sequence"
        )
    async with self.authority.read(ctx) as view:
        await require_namespace_in(view, ctx)
        statement = select(source_operations).where(
            source_operations.c.namespace_id == ctx.namespace_id,
            
        )
        if source_operation_id is not None:
            statement = statement.where(source_operations.c.id == source_operation_id)
        else:
            statement = statement.where(
                source_operations.c.record["result"]["commit_seq"].as_integer() == commit_seq
            )
        rows = (await view.connection.execute(statement)).mappings().all()
        if not rows:
            raise DomainError("source_operation_not_found", "Source operation is unavailable")
        if len(rows) != 1:
            raise DomainError(
                "source_operation_ambiguous",
                "This commit contains multiple source operations; select a log identity",
            )
        row = rows[0]
        await self.authorize_result_in(
            view, ctx, SourceResult.model_validate(row["record"]["result"])
        )
        return {"source_operation_id": str(row["id"]), **row["record"]}


async def list_operations(
    self: SourceService, ctx: CallContext, limit: int = 100, before_commit_seq: int | None = None
):
    if not 1 <= limit <= 1000:
        raise DomainError("invalid_limit", "Source operation limit must be between 1 and 1000")
    async with self.authority.read(ctx) as view:
        await require_namespace_in(view, ctx)
        sequence = source_operations.c.record["result"]["commit_seq"].as_integer()
        statement = select(source_operations).where(
            source_operations.c.namespace_id == ctx.namespace_id,
            
        )
        if before_commit_seq is not None:
            statement = statement.where(sequence < before_commit_seq)
        statement = statement.order_by(sequence.desc(), source_operations.c.id.desc())
        rows = (await view.connection.execute(statement)).mappings()
        result: list[dict[str, JsonValue]] = []
        for row in rows:
            value = row["record"]
            permitted = True
            try:
                await self.authorize_result_in(
                    view, ctx, SourceResult.model_validate(value["result"])
                )
            except DomainError as error:
                if error.code not in {"source_unavailable", "cognition_not_found"}:
                    raise
                permitted = False
            if permitted:
                result.append({"source_operation_id": str(row["id"]), **value})
            if len(result) >= limit:
                break
        return result
