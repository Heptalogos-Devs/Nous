"""Resolve projections through the same exact source and cognition owners."""

import math
from hashlib import sha256
from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, CognitiveRef, DomainError
from nous_sdk.presentation import body_text
from sqlalchemy import select

from nous.cognition.lifecycle import selectable
from nous.cognition.schema import revisions
from nous.material.service import observations
from nous.platform.models.embedding import vector_space
from nous.platform.persistence import ReadView, WriteTx
from nous.platform.persistence.authority import canonical_json
from nous.retrieval.media import embedding_input, media_records_in, source_material_ids_in

from .contracts import Content, Input, Target
from .schema import contents

if TYPE_CHECKING:
    from nous.platform.models.embedding_profile import EmbeddingProfile

    from .service import VectorProjectionService


async def resolve_input(
    self: VectorProjectionService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    target: Target,
    resource: str,
    *,
    head_only: bool,
) -> Input | None:
    text = None
    media = None
    if isinstance(target, CognitiveRef):
        record = await self.cognition.get_in(view, ctx, target)
        if not selectable(record, include_inactive=not head_only):
            return None
        if head_only and not await view.connection.scalar(
            select(revisions.c.head).where(
                revisions.c.id == target.id,
                revisions.c.revision == target.revision,
                revisions.c.namespace_id == ctx.namespace_id,
            )
        ):
            return None
        text = body_text(record.body)
        if not text.strip():
            return None
        # get_in already validates every exact evidence/media source requirement.
        # Roots are the canonical provenance closure, not caller-provided labels.
        source_ids = tuple(
            sorted(
                set(
                    (
                        await view.connection.execute(
                            select(observations.c.material_id).where(
                                observations.c.namespace_id == ctx.namespace_id,
                                observations.c.id.in_(record.source_roots),
                            )
                        )
                    ).scalars()
                )
            )
        )
    else:
        selected = [
            record
            for record in await media_records_in(
                view, ctx, self.materials, self.derivations, include_text=True
            )
            if record.reference.exact() == target.exact()
            and (
                target.observation_id is None
                or (
                    record.reference.observation_id == target.observation_id
                    if target.kind in {"material", "source_region"}
                    else target.observation_id in record.source_roots
                )
            )
        ]
        if not selected:
            raise DomainError("source_unavailable", "Projection media source is unavailable")
        record = max(selected, key=lambda item: item.commit_seq)
        content = embedding_input(record)
        if isinstance(content, str):
            text = content
        else:
            media = content
        source_ids = await source_material_ids_in(
            view, ctx, target, self.materials, self.derivations
        )
    key = sha256(
        canonical_json(
            (
                ctx.namespace_id,
                ctx.namespace_id,
                resource,
                text,
                media.model_dump(mode="json") if media else None,
                source_ids,
            )
        ).encode()
    ).hexdigest()
    return Input(key=key, text=text, media=media, source_ids=source_ids)


async def retained_content_in(
    self: VectorProjectionService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    target: Target,
    value: Input,
    profile: EmbeddingProfile,
    *,
    repair: bool = False,
) -> Content | None:
    raw = await view.connection.scalar(
        select(contents.c.record).where(
            contents.c.namespace_id == ctx.namespace_id,
            contents.c.key == value.key,
        )
    )
    if raw is not None:
        content = Content.model_validate(raw)
        current_input = content.input.model_dump(mode="json")
        expected_input = value.model_dump(mode="json")
        route_matches = any(
            content.dimensions == route.dimensions
            and content.space == vector_space(route, content.model)
            for route in (profile.route, *profile.fallbacks)
        )
        vector_valid = len(content.vector) == content.dimensions and all(
            math.isfinite(component) for component in content.vector
        )
        if (
            current_input == expected_input
            and route_matches
            and vector_valid
            and content.producer.task == "embedding"
            and content.producer.model_id == content.model
        ):
            return content
        if not repair:
            raise DomainError(
                "embedding_cache_conflict",
                "Cached vector result differs from its exact input or space",
            )
        await view.connection.execute(
            contents.delete().where(
                contents.c.namespace_id == ctx.namespace_id,
                contents.c.key == value.key,
            )
        )
    return None
