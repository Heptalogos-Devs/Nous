"""Read source operations by their actual log identity or committed sequence."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, DomainError
from pydantic import JsonValue
from sqlalchemy import select

from nous.material.service import require_observation_in

from .schema import source_operations

if TYPE_CHECKING:
    from .sources import SourceService


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
        statement = select(source_operations).where(
            source_operations.c.namespace_id == ctx.namespace_id,
            source_operations.c.subject_id == ctx.subject_id,
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
        for observation_id in row["record"]["result"]["observation_ids"]:
            await require_observation_in(view, ctx, observation_id, active=False)
        return {"source_operation_id": str(row["id"]), **row["record"]}


async def list_operations(
    self: SourceService, ctx: CallContext, limit: int = 100, before_commit_seq: int | None = None
):
    if not 1 <= limit <= 1000:
        raise DomainError("invalid_limit", "Source operation limit must be between 1 and 1000")
    async with self.authority.read(ctx) as view:
        sequence = source_operations.c.record["result"]["commit_seq"].as_integer()
        statement = select(source_operations).where(
            source_operations.c.namespace_id == ctx.namespace_id,
            source_operations.c.subject_id == ctx.subject_id,
        )
        if before_commit_seq is not None:
            statement = statement.where(sequence < before_commit_seq)
        statement = statement.order_by(sequence.desc(), source_operations.c.id.desc())
        rows = (await view.connection.execute(statement)).mappings()
        result: list[dict[str, JsonValue]] = []
        for row in rows:
            value = row["record"]
            permitted = True
            for observation_id in value["result"]["observation_ids"]:
                try:
                    await require_observation_in(view, ctx, observation_id, active=False)
                except DomainError as error:
                    if error.code != "source_unavailable":
                        raise
                    permitted = False
                    break
            if permitted:
                result.append({"source_operation_id": str(row["id"]), **value})
            if len(result) >= limit:
                break
        return result
