"""Derive review state from explicit current-basis references and corrections."""

from collections.abc import Sequence

import sqlalchemy as sa

from nous.storage.database import ReadTx, WriteTx

from . import feedback_schema, schema


def _dependents(seeds: sa.Select[tuple[str]]) -> sa.CTE:
    affected = seeds.cte("affected_bases", recursive=True)
    return affected.union(
        sa.select(schema.sources.c.revision_id.label("revision_id"))
        .join(affected, schema.sources.c.target_revision_id == affected.c.revision_id)
        .where(schema.sources.c.usage == "current_basis")
    )


async def needs_review_in(tx: ReadTx, revisions: Sequence[str]) -> set[str]:
    if not revisions:
        return set()
    affected = _dependents(sa.select(feedback_schema.invalidations.c.revision_id))
    return set(
        (
            await tx.connection.scalars(
                sa.select(affected.c.revision_id).where(affected.c.revision_id.in_(revisions))
            )
        ).all()
    )


async def invalidate_in(
    tx: WriteTx, revision_id: str, corrected_revision_id: str, reason: str | None, recorded_at: str
) -> bool:
    await tx.connection.execute(
        sa.insert(feedback_schema.invalidations).values(
            revision_id=revision_id,
            corrected_revision_id=corrected_revision_id,
            reason=reason,
            recorded_at=recorded_at,
        )
    )
    affected = _dependents(
        sa.select(feedback_schema.invalidations.c.revision_id).where(
            feedback_schema.invalidations.c.revision_id == revision_id
        )
    )
    return bool(
        await tx.connection.scalar(
            sa.select(
                sa.exists().where(
                    schema.records.c.head_revision_id.in_(sa.select(affected.c.revision_id)),
                    schema.records.c.recall.is_(True),
                    schema.records.c.archived.is_(False),
                )
            )
        )
    )
