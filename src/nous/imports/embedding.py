"""Import vectors use the durable projection owner and its actual job results."""

from __future__ import annotations

from typing import TYPE_CHECKING
from uuid import uuid4

from nous_wave.contracts import CallContext, CognitiveRecord, CognitiveRef, DomainError, UnitOutput
from nous_wave.contracts.imports import EmbeddingSyncResult, VectorAdoption
from nous_wave.contracts.vector_projection import ProjectionReceipt, ProjectionResult
from sqlalchemy import select

from nous.cognition.schema import revisions

from .schema import outputs

if TYPE_CHECKING:
    from .service import ImportService


def adoption(result: ProjectionResult) -> VectorAdoption:
    if len(result.spaces) > 1:
        raise DomainError(
            "embedding_space_conflict", "One projection resolved to multiple embedding spaces"
        )
    return VectorAdoption(
        embedded=len(result.references),
        space=result.spaces[0] if result.spaces else None,
        references=result.references,
    )


async def embed_records(
    self: ImportService,
    ctx: CallContext,
    references: tuple[CognitiveRef, ...],
    operation_id: str,
) -> VectorAdoption:
    if self.projection is None:
        raise RuntimeError("The vector projection owner is not composed")
    return adoption(await self.projection.project(ctx, references, operation_id))


async def embed_output(
    self: ImportService,
    ctx: CallContext,
    output_id: str,
    *,
    required: bool = True,
    operation_id: str | None = None,
    receipt: ProjectionReceipt | None = None,
) -> VectorAdoption:
    async with self.authority.read(ctx) as view:
        value = await view.connection.scalar(
            select(outputs.c.record).where(outputs.c.id == output_id)
        )
        if value is None:
            raise DomainError("output_not_found", "Import output does not exist")
        output = UnitOutput.model_validate(value)
        for reference in output.cognition:
            await self.cognition.get_in(view, ctx, reference)
    if self.projection is None:
        raise RuntimeError("The vector projection owner is not composed")
    if receipt is not None:
        result = adoption(
            ProjectionResult.model_validate(
                await self.projection.execution().wait(ctx, receipt.job_id)
            )
        )
    else:
        result = await self.embed_records(
            ctx, output.cognition, operation_id or output_id + ":embed"
        )
    return result.model_copy(update={"output_id": output_id})


async def embedding_sync(
    self: ImportService,
    ctx: CallContext,
    limit: int = 100,
    operation_id: str | None = None,
) -> EmbeddingSyncResult:
    if not 1 <= limit <= 10000:
        raise DomainError("invalid_limit", "Embedding sync limit must be between 1 and 10000")
    if self.projection is None:
        raise RuntimeError("The vector projection owner is not composed")
    async with self.authority.read(ctx) as view:
        values = (
            await view.connection.execute(
                select(revisions.c.record)
                .where(
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.subject_id == ctx.require_subject(),
                    revisions.c.head.is_(True),
                )
                .order_by(revisions.c.commit_seq, revisions.c.id)
            )
        ).scalars()
        records: list[CognitiveRecord] = []
        for value in values:
            record = CognitiveRecord.model_validate(value)
            if (
                not record.deleted
                and not record.purged
                and await self.cognition.visible_in(view, ctx, record)
            ):
                records.append(record)
    missing = await self.projection.missing(ctx, tuple(record.reference for record in records))
    selected = missing[:limit]
    if not selected:
        return EmbeddingSyncResult(embedded=0, remaining=0, space=None)
    result = await self.embed_records(ctx, selected, operation_id or str(uuid4()))
    return EmbeddingSyncResult(
        embedded=result.embedded, remaining=len(missing) - len(selected), space=result.space
    )
