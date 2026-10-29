"""Adopt actual provider spaces for exact, currently authorized cognitive revisions."""

from __future__ import annotations

import math
from typing import TYPE_CHECKING

from nous_wave.contracts import (
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    DomainError,
    EmbeddingBatch,
    UnitOutput,
)
from nous_wave.contracts.imports import EmbeddingSyncResult, VectorAdoption
from sqlalchemy import select

from nous.cognition.projection import body_text, media_references_in
from nous.cognition.schema import revisions, vectors
from nous.material.service import observations

from .schema import outputs

if TYPE_CHECKING:
    from .service import ImportService


async def adopt_vectors(
    self: ImportService,
    ctx: CallContext,
    records: tuple[CognitiveRecord, ...],
    batch: EmbeddingBatch,
    operation_id: str,
) -> VectorAdoption:
    if len(batch.vectors) != len(records) or any(
        len(vector) != batch.dimensions or not all(math.isfinite(x) for x in vector)
        for vector in batch.vectors
    ):
        raise DomainError(
            "invalid_embedding", "Embedding response has invalid count, dimensions, or coordinates"
        )
    async with self.authority.write(
        ctx,
        operation_id + ":" + batch.space,
        operation_kind="cognition.embed",
        scope=ctx.require_subject(),
        payload={
            "references": [record.reference.model_dump(mode="json") for record in records],
            "space": batch.space,
            "model": batch.model,
        },
    ) as tx:
        if tx.replayed:
            return VectorAdoption.model_validate(tx.replay_result)
        inserted = 0
        for record, vector, call_id in zip(records, batch.vectors, batch.call_ids, strict=True):
            await self.cognition.get_in(tx, ctx, record.reference)
            existing = (
                await tx.connection.execute(
                    select(vectors.c.model, vectors.c.dimensions).where(
                        vectors.c.reference == record.reference.exact(),
                        vectors.c.space == batch.space,
                    )
                )
            ).first()
            if existing is not None:
                if existing.model != batch.model or existing.dimensions != batch.dimensions:
                    raise DomainError(
                        "embedding_space_conflict",
                        "An embedding space cannot mix models or dimensions",
                    )
                continue
            await tx.connection.execute(
                vectors.insert().values(
                    reference=record.reference.exact(),
                    space=batch.space,
                    model=batch.model,
                    dimensions=batch.dimensions,
                    vector=list(vector),
                    call_id=call_id,
                    commit_seq=tx.commit_seq,
                )
            )
            inserted += 1
        return tx.result(
            VectorAdoption(
                embedded=inserted,
                space=batch.space,
                references=tuple(record.reference.exact() for record in records),
            )
        )


async def embed_records(
    self: ImportService, ctx: CallContext, references: tuple[CognitiveRef, ...], operation_id: str
):
    async with self.authority.read(ctx) as view:
        records = tuple(
            [await self.cognition.get_in(view, ctx, reference) for reference in references]
        )
        source_ids = tuple(
            sorted(
                set(
                    (
                        await view.connection.execute(
                            select(observations.c.material_id).where(
                                observations.c.id.in_(
                                    {root for record in records for root in record.source_roots}
                                )
                            )
                        )
                    ).scalars()
                )
            )
        )
        all_sources = set(source_ids)
        internal = ctx.model_copy(update={"allowed_source_ids": None, "source_constraints": ()})
        for record in records:
            for media in media_references_in(record.body):
                if media.kind in {"representation", "derived_region"}:
                    all_sources.update(
                        (
                            await self.cognition.derivations.source_in(view, internal, media)
                        ).material_ids
                    )
                else:
                    all_sources.update(
                        (
                            await self.materials.provenance_in(
                                view, internal, media, derivations=self.cognition.derivations
                            )
                        ).material_ids
                    )
        source_ids = tuple(sorted(all_sources))
    if not records:
        return VectorAdoption(embedded=0, space=None)
    batch = await self.models.embed(
        ctx.model_copy(update={"request_id": operation_id}),
        tuple(body_text(record.body) for record in records),
        source_ids=source_ids,
    )
    return await adopt_vectors(self, ctx, records, batch, operation_id)


async def embed_output(
    self: ImportService, ctx: CallContext, output_id: str, *, required: bool = True
) -> VectorAdoption:
    async with self.authority.read(ctx) as view:
        value = await view.connection.scalar(
            select(outputs.c.record).where(outputs.c.id == output_id)
        )
        if value is None:
            raise DomainError("output_not_found", "Import output does not exist")
        output = UnitOutput.model_validate(value)
    result = await embed_records(self, ctx, output.cognition, output_id + ":embed")
    return result.model_copy(update={"output_id": output_id})


async def embedding_sync(
    self: ImportService, ctx: CallContext, limit: int = 100, operation_id: str | None = None
):
    from uuid import uuid4

    if not 1 <= limit <= 10000:
        raise DomainError("invalid_limit", "Embedding sync limit must be between 1 and 10000")
    operation_id = operation_id or str(uuid4())
    async with self.authority.read(ctx) as view:
        values = (
            await view.connection.execute(
                select(revisions.c.record)
                .where(
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.subject_id == ctx.subject_id,
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
        if not records:
            return EmbeddingSyncResult(embedded=0, remaining=0, space=None)
        first = records[0]
        source_ids = tuple(
            str(value)
            for value in (
                await view.connection.execute(
                    select(observations.c.material_id).where(
                        observations.c.id.in_(first.source_roots)
                    )
                )
            ).scalars()
        )
    # Resolve the effective space from a real response; the model ledger reuses its
    # actual paid snapshot. Equal dimensions never authorize cross-model reuse.
    probe = await self.models.embed(
        ctx.model_copy(update={"request_id": operation_id + ":space"}),
        (body_text(first.body),),
        source_ids=source_ids,
    )
    async with self.authority.read(ctx) as view:
        indexed = {
            str(row.reference)
            for row in (
                await view.connection.execute(
                    select(vectors.c.reference).where(
                        vectors.c.space == probe.space,
                        vectors.c.model == probe.model,
                        vectors.c.dimensions == probe.dimensions,
                    )
                )
            ).all()
        }
    missing = [record for record in records if record.reference.exact() not in indexed]
    selected = missing[:limit]
    if not selected:
        return EmbeddingSyncResult(embedded=0, remaining=0, space=probe.space)
    if selected[0].reference == first.reference:
        await adopt_vectors(self, ctx, (first,), probe, operation_id + ":probe")
        selected = selected[1:]
    if selected:
        await embed_records(
            self, ctx, tuple(record.reference for record in selected), operation_id + ":batch"
        )
    return EmbeddingSyncResult(
        embedded=min(len(missing), limit), remaining=max(0, len(missing) - limit), space=probe.space
    )
