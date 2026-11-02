"""Metadata-only vector inventory under the same actual source permission rules."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, CognitiveRecord, DomainError
from nous_sdk.contracts.diagnostics import RetrievalStatus, VectorSpaceInventory
from sqlalchemy import select

from nous.cognition.lifecycle import selectable
from nous.cognition.schema import revisions, vectors
from nous.identity.access import require_namespace_in

from .media import media_records_in

if TYPE_CHECKING:
    from .service import RetrievalService


async def status(self: RetrievalService, ctx: CallContext) -> RetrievalStatus:
    profile = None
    try:
        profile = await self.models.prepare_embedding(ctx)
    except DomainError as error:
        if error.code != "model_unavailable":
            raise
    async with self.authority.read(ctx) as view:
        namespace = await require_namespace_in(view, ctx)
        configured = (
            await self.models.configured_embedding_spaces_in(view, ctx, profile) if profile else ()
        )
        values = (
            await view.connection.execute(
                select(revisions.c.record).where(
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.head.is_(True),
                )
            )
        ).scalars()
        eligible: set[str] = set()
        for value in values:
            record = CognitiveRecord.model_validate(value)
            if record.reference.kind == "episode" and "episodes" not in namespace.enabled_features:
                continue
            if selectable(record) and await self.cognition.visible_in(view, ctx, record):
                eligible.add(record.reference.exact())
        for media in await media_records_in(
            view, ctx, self.materials, self.derivations, include_text=False
        ):
            eligible.add(media.reference.exact())
        grouped: dict[tuple[str, str, int], set[str]] = {}
        rows = (
            await view.connection.execute(
                select(
                    vectors.c.reference, vectors.c.space, vectors.c.model, vectors.c.dimensions
                ).where(vectors.c.reference.in_(eligible))
            )
        ).all()
        for row in rows:
            grouped.setdefault((str(row.space), str(row.model), int(row.dimensions)), set()).add(
                str(row.reference)
            )
        inventories = tuple(
            VectorSpaceInventory(
                space=space,
                model=model,
                dimensions=dimension,
                indexed_references=len(references),
                missing_references=len(eligible - references),
            )
            for (space, model, dimension), references in sorted(grouped.items())
        )
        generations = sum(
            set(snapshot.references).issubset(eligible) for snapshot in self.snapshots.values()
        )
        return RetrievalStatus(
            authority_commit_seq=view.commit_seq,
            visible_targets=len(eligible),
            native_generations=generations,
            spaces=inventories,
            configured_space_complete=(
                not eligible
                or any(
                    eligible.issubset(
                        grouped.get((space.space, space.model, space.dimensions), set())
                    )
                    for space in configured
                )
            )
            if configured
            else None,
        )
