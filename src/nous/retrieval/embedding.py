from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import (
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
    concepts: tuple[str, ...] = (),
) -> EmbeddingBatch:
    parts = request.parts or (request.text, *request.media)
    media_parts = tuple(part for part in parts if isinstance(part, MediaReference))
    if (
        len(media_parts) == 1
        and not concepts
        and not any(isinstance(part, str) and part.strip() for part in parts)
    ):
        # A saved media cue already has a producer and semantic space. Reuse it
        # without invoking a new model, including when providers are disabled.
        reference = media_parts[0]
        async with self.authority.read(ctx) as view:
            await source_material_ids_in(view, ctx, reference, self.materials, self.derivations)
            saved = (
                (
                    await view.connection.execute(
                        select(vectors)
                        .where(vectors.c.reference == reference.exact())
                        .order_by(vectors.c.commit_seq.desc(), vectors.c.space)
                    )
                )
                .mappings()
                .first()
            )
            if saved is not None:
                return EmbeddingBatch(
                    model=saved["model"],
                    space=saved["space"],
                    dimensions=saved["dimensions"],
                    vectors=(tuple(saved["vector"]),),
                    call_ids=(saved["call_id"],),
                )
    if not any(isinstance(part, MediaReference) for part in parts):
        return await self.models.embed(
            ctx.model_copy(update={"request_id": f"{ctx.request_id}:query"}),
            ("\n".join(part for part in parts if isinstance(part, str)), *concepts),
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
        (tuple(converted), *((text,) for text in concepts)),
        source_ids=tuple(sorted(input_sources)),
    )


async def embed_media(
    self: RetrievalService, ctx: CallContext, reference: MediaReference, operation_id: str
) -> MediaEmbeddingResult:
    if self.projection is None:
        raise RuntimeError("The vector projection owner is not composed")
    result = await self.projection.project(ctx, (reference,), operation_id)
    if len(result.spaces) != 1 or reference.exact() not in result.references:
        raise DomainError("source_unavailable", "The media projection has no available vector")
    async with self.authority.read(ctx) as view:
        await source_material_ids_in(view, ctx, reference, self.materials, self.derivations)
        row = (
            (
                await view.connection.execute(
                    select(vectors).where(
                        vectors.c.reference == reference.exact(),
                        vectors.c.space == result.spaces[0],
                    )
                )
            )
            .mappings()
            .one()
        )
        return MediaEmbeddingResult(
            reference=reference,
            space=str(row["space"]),
            model=str(row["model"]),
            dimensions=int(row["dimensions"]),
            call_ids=(str(row["call_id"]),),
            commit_seq=int(row["commit_seq"]),
        )
