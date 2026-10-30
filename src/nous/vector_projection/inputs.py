"""Resolve projections through the same exact source and cognition owners."""

from hashlib import sha256
from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, CognitiveRef, DomainError
from sqlalchemy import select

from nous.cognition.lifecycle import selectable
from nous.cognition.projection import body_text
from nous.cognition.schema import revisions, vectors
from nous.material.service import observations
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
                revisions.c.subject_id == ctx.require_subject(),
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
                                observations.c.subject_id == ctx.require_subject(),
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
                view, ctx, self.materials, self.derivations, include_text=False
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
        media = embedding_input(record)
        source_ids = await source_material_ids_in(
            view, ctx, target, self.materials, self.derivations
        )
    key = sha256(
        canonical_json(
            (
                ctx.namespace_id,
                ctx.require_subject(),
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
) -> Content | None:
    raw = await view.connection.scalar(select(contents.c.record).where(contents.c.key == value.key))
    if raw is not None:
        content = Content.model_validate(raw)
        await self.models.successful_embedding_in(view, ctx, content.call_id)
        return content
    references = [target.exact()]
    if isinstance(target, CognitiveRef):
        references.extend(
            CognitiveRef(kind=target.kind, id=target.id, revision=int(revision)).exact()
            for revision in (
                await view.connection.execute(
                    select(revisions.c.revision).where(
                        revisions.c.id == target.id,
                        revisions.c.namespace_id == ctx.namespace_id,
                        revisions.c.subject_id == ctx.require_subject(),
                        revisions.c.revision < target.revision,
                    )
                )
            ).scalars()
        )
    rows = (
        await view.connection.execute(
            select(vectors)
            .where(
                vectors.c.reference.in_(references),
                vectors.c.call_id.is_not(None),
            )
            .order_by(vectors.c.commit_seq.desc())
        )
    ).mappings()
    for row in rows:
        evidence = await self.models.reusable_embedding_in(
            view,
            ctx,
            str(row["call_id"]),
            profile,
            text=value.text,
            media=value.media,
            source_ids=value.source_ids,
        )
        if evidence is None:
            continue
        batch = evidence.batch
        if (row["space"], row["model"], row["dimensions"], tuple(row["vector"])) != (
            batch.space,
            batch.model,
            batch.dimensions,
            batch.vectors[0],
        ):
            raise DomainError(
                "embedding_producer_conflict", "The adopted vector differs from its paid result"
            )
        return Content(
            input=value,
            space=batch.space,
            model=batch.model,
            dimensions=batch.dimensions,
            vector=batch.vectors[0],
            call_id=str(row["call_id"]),
            call_source_ids=evidence.source_ids,
        )
    return None
