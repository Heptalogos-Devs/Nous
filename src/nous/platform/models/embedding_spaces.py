"""Associate configured routes with spaces proved by retained physical responses."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, DomainError
from sqlalchemy import func, select

from nous.platform.configuration import EmbeddingRoute
from nous.platform.persistence import ReadView, WriteTx

from .embedding import retained_embedding, vector_space
from .embedding_profile import EmbeddingProfile, EmbeddingSpaceEvidence
from .history import imported_calls
from .ledger import calls

if TYPE_CHECKING:
    from . import ModelService


async def observed_spaces_in(
    self: ModelService, view: ReadView | WriteTx, ctx: CallContext, route: EmbeddingRoute
) -> tuple[EmbeddingSpaceEvidence, ...]:
    result: dict[str, EmbeddingSpaceEvidence] = {}
    for table, request, output, status, purpose in (
        (calls, calls.c.request, calls.c.result, calls.c.status, calls.c.purpose),
        (
            imported_calls,
            imported_calls.c.record["request"],
            imported_calls.c.record["result"],
            imported_calls.c.record["status"].as_string(),
            imported_calls.c.record["purpose"].as_string(),
        ),
    ):
        query = (
            select(func.min(table.c.id), table.c.namespace_id)
            .where(
                table.c.namespace_id == ctx.namespace_id,
                status == "succeeded",
                purpose == "embedding",
                request["model"].as_string() == route.model,
                request["base_url"].as_string() == route.base_url,
                request["dimensions"].as_integer() == route.dimensions,
                request["normalization"].as_string() == route.normalization,
            )
            .group_by(table.c.namespace_id, request["extra_body"], output["space"])
        )
        if ctx.namespace_id is not None:
            query = query.where(table.c.namespace_id == ctx.namespace_id)
        for call_id, namespace_id in (await view.connection.execute(query)).all():
            if not namespace_id:
                continue
            try:
                paid = await self.ledger.successful_in(
                    view, ctx.model_copy(update={}), str(call_id)
                )
            except DomainError as error:
                if error.code in {"model_call_forbidden", "model_call_unavailable"}:
                    continue
                raise
            if not EmbeddingProfile.matches_request(route, paid.request):
                continue
            batch = retained_embedding(paid.result, paid.origin_call_id or paid.id)
            if batch.dimensions == route.dimensions and batch.space == vector_space(
                route, batch.model
            ):
                result[batch.space] = EmbeddingSpaceEvidence(
                    batch.space, batch.model, batch.dimensions
                )
    return tuple(result[key] for key in sorted(result))


async def configured_spaces_in(
    self: ModelService, view: ReadView | WriteTx, ctx: CallContext, profile: EmbeddingProfile
) -> tuple[EmbeddingSpaceEvidence, ...]:
    """A route's actual response names take precedence over its unobserved alias."""
    result: dict[str, EmbeddingSpaceEvidence] = {}
    for index, route in enumerate((profile.route, *profile.fallbacks)):
        if not route.model or not route.base_url:
            continue
        observed = await observed_spaces_in(self, view, ctx, route)
        for space in observed or (
            EmbeddingSpaceEvidence(vector_space(route, route.model), route.model, route.dimensions),
        ):
            indexes = (
                (*result[space.space].route_indexes, index) if space.space in result else (index,)
            )
            result[space.space] = EmbeddingSpaceEvidence(
                space.space, space.model, space.dimensions, indexes
            )
    return tuple(result[key] for key in sorted(result))
