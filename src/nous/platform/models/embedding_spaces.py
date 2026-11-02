"""Report configured embedding spaces and spaces used by real vector projections."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext
from sqlalchemy import select

from nous.platform.configuration import EmbeddingRoute
from nous.platform.persistence import ReadView, WriteTx

from .embedding import vector_space
from .embedding_profile import EmbeddingProfile, EmbeddingSpaceEvidence

if TYPE_CHECKING:
    from . import ModelService


async def observed_spaces_in(
    self: ModelService, view: ReadView | WriteTx, ctx: CallContext, route: EmbeddingRoute
) -> tuple[EmbeddingSpaceEvidence, ...]:
    from nous.cognition.schema import vectors

    found: dict[str, EmbeddingSpaceEvidence] = {}
    rows = (
        await view.connection.execute(
            select(vectors.c.space, vectors.c.model, vectors.c.dimensions).where(
                vectors.c.namespace_id == ctx.namespace_id
            )
        )
    ).all()
    for space, model, dimensions in rows:
        if (
            not isinstance(model, str)
            or not isinstance(space, str)
            or not isinstance(dimensions, int)
            or dimensions != route.dimensions
            or vector_space(route, model) != space
        ):
            continue
        found[space] = EmbeddingSpaceEvidence(space, model, int(dimensions))
    return tuple(found[key] for key in sorted(found))


async def configured_spaces_in(
    self: ModelService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    profile: EmbeddingProfile,
) -> tuple[EmbeddingSpaceEvidence, ...]:
    result: dict[str, EmbeddingSpaceEvidence] = {}
    for index, route in enumerate((profile.route, *profile.fallbacks)):
        if not route.model or not route.base_url:
            continue
        observed = await observed_spaces_in(self, view, ctx, route)
        spaces = observed or (
            EmbeddingSpaceEvidence(vector_space(route, route.model), route.model, route.dimensions),
        )
        for space in spaces:
            indexes = (*result[space.space].route_indexes, index) if space.space in result else (index,)
            result[space.space] = EmbeddingSpaceEvidence(
                space.space, space.model, space.dimensions, indexes
            )
    return tuple(result[key] for key in sorted(result))
