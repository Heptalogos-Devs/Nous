from __future__ import annotations

from typing import TYPE_CHECKING

import numpy as np
from nous_wave.contracts import (
    CallContext,
    DomainError,
    EmbeddingBatch,
    MediaEmbeddingInput,
    MediaEmbeddingResult,
    MediaReference,
    QueryRequest,
)
from sqlalchemy import select

from nous.cognition.schema import vectors

from .media import embedding_input, media_records_in, source_material_ids_in

if TYPE_CHECKING:
    from .service import RetrievalService


async def query_embedding(
    self: RetrievalService,
    ctx: CallContext,
    request: QueryRequest,
    *,
    source_ids: tuple[str, ...] = (),
) -> EmbeddingBatch:
    parts = request.parts or (request.text, *request.media)
    if not any(isinstance(part, MediaReference) for part in parts):
        return await self.models.embed(
            ctx.model_copy(update={"request_id": f"{ctx.request_id}:query"}),
            ("\n".join(part for part in parts if isinstance(part, str)),),
            source_ids=source_ids,
        )
    async with self.authority.read(ctx) as view:
        available = await media_records_in(view, ctx, self.materials, self.derivations)
    converted: list[str | MediaEmbeddingInput] = []
    input_sources: set[str] = set(source_ids)
    for part in parts:
        if isinstance(part, str):
            converted.append(part)
            continue
        matched = [
            record
            for record in available
            if record.reference.exact() == part.exact()
            and (
                part.observation_id is None
                or (
                    record.reference.observation_id == part.observation_id
                    if part.kind in {"material", "source_region"}
                    else part.observation_id in record.source_roots
                )
            )
        ]
        if not matched:
            raise DomainError(
                "source_unavailable", "Query media is outside the current source scope"
            )
        record = max(matched, key=lambda item: item.commit_seq)
        converted.append(embedding_input(record))
        async with self.authority.read(ctx) as view:
            input_sources.update(
                await source_material_ids_in(
                    view, ctx, record.reference, self.materials, self.derivations
                )
            )
    return await self.models.embed_media(
        ctx.model_copy(update={"request_id": f"{ctx.request_id}:query-media"}),
        (tuple(converted),),
        source_ids=tuple(sorted(input_sources)),
    )


async def embed_media(
    self: RetrievalService, ctx: CallContext, reference: MediaReference, operation_id: str
) -> MediaEmbeddingResult:
    async with self.authority.read(ctx) as view:
        candidates = [
            record
            for record in await media_records_in(view, ctx, self.materials, self.derivations)
            if record.reference.exact() == reference.exact()
            and (
                reference.observation_id is None
                or (
                    record.reference.observation_id == reference.observation_id
                    if reference.kind in {"material", "source_region"}
                    else reference.observation_id in record.source_roots
                )
            )
        ]
    if not candidates:
        raise DomainError("source_unavailable", "Media embedding source is unavailable")
    record = max(candidates, key=lambda item: item.commit_seq)
    async with self.authority.read(ctx) as view:
        source_ids = await source_material_ids_in(
            view, ctx, record.reference, self.materials, self.derivations
        )
    receipt = await self.authority.receipt(
        ctx,
        operation_id,
        operation_kind="media.embed",
        scope=ctx.require_subject(),
        payload=reference,
    )
    if receipt is not None:
        return MediaEmbeddingResult.model_validate(receipt.result)
    batch = await self.models.embed_media(
        ctx.model_copy(update={"request_id": operation_id}),
        ((embedding_input(record),),),
        source_ids=source_ids,
    )
    if (
        len(batch.vectors) != 1
        or len(batch.vectors[0]) != batch.dimensions
        or not np.isfinite(batch.vectors[0]).all()
    ):
        raise DomainError(
            "invalid_embedding", "Media embedding has invalid count, dimensions, or coordinates"
        )
    async with self.authority.write(
        ctx,
        operation_id,
        operation_kind="media.embed",
        scope=ctx.require_subject(),
        payload=reference,
    ) as tx:
        await source_material_ids_in(tx, ctx, record.reference, self.materials, self.derivations)
        if tx.replayed:
            return MediaEmbeddingResult.model_validate(tx.replay_result)
        existing = (
            await tx.connection.execute(
                select(vectors.c.model, vectors.c.dimensions).where(
                    vectors.c.reference == reference.exact(), vectors.c.space == batch.space
                )
            )
        ).first()
        if existing is not None and (
            existing.model != batch.model or existing.dimensions != batch.dimensions
        ):
            raise DomainError(
                "embedding_space_conflict",
                "A media space cannot mix model identities or dimensions",
            )
        if existing is None:
            await tx.connection.execute(
                vectors.insert().values(
                    reference=reference.exact(),
                    space=batch.space,
                    model=batch.model,
                    dimensions=batch.dimensions,
                    vector=list(batch.vectors[0]),
                    call_id=batch.call_ids[0],
                    commit_seq=tx.commit_seq,
                )
            )
        return tx.result(
            MediaEmbeddingResult(
                reference=reference,
                space=batch.space,
                model=batch.model,
                dimensions=batch.dimensions,
                call_ids=batch.call_ids,
                commit_seq=tx.commit_seq,
            )
        )
