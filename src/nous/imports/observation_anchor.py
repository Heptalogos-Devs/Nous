"""Locate the actual literal anchor of an accepted source event without copying it."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.contracts import CallContext, CognitiveRecord, DomainError, MemoryBody
from nous_wave.contracts.imports import FormObservation
from sqlalchemy import exists, func, select

from nous.cognition.schema import revisions
from nous.platform.persistence import ReadView

if TYPE_CHECKING:
    from .service import ImportService


async def anchor_in(
    self: ImportService,
    view: ReadView,
    ctx: CallContext,
    command: FormObservation,
    *,
    region_id: str | None = None,
    text: str | None = None,
) -> CognitiveRecord | None:
    if command.anchor is not None:
        candidates = [await self.cognition.get_in(view, ctx, command.anchor)]
    else:
        evidence = func.json_each(revisions.c.record, "$.evidence").table_valued("value")
        candidates = [
            CognitiveRecord.model_validate(value)
            for value in (
                await view.connection.execute(
                    select(revisions.c.record)
                    .where(
                        revisions.c.namespace_id == ctx.namespace_id,
                        revisions.c.subject_id == ctx.subject_id,
                        revisions.c.kind == "memory",
                        revisions.c.head.is_(True),
                        exists(
                            select(1)
                            .select_from(evidence)
                            .where(
                                func.json_extract(evidence.c.value, "$.observation_id")
                                == command.observation_id
                            )
                        ),
                    )
                    .order_by(revisions.c.commit_seq, revisions.c.id)
                )
            ).scalars()
        ]
    matches: list[CognitiveRecord] = []
    for candidate in candidates:
        if (
            not isinstance(candidate.body, MemoryBody)
            or candidate.deleted
            or candidate.purged
            or (text is not None and candidate.body.text != text)
            or not any(
                link.observation_id == command.observation_id
                and (region_id is None or link.region_id == region_id)
                for link in candidate.evidence
            )
            or not await self.cognition.visible_in(view, ctx, candidate)
        ):
            continue
        matches.append(candidate)
    if command.anchor is not None and not matches:
        raise DomainError(
            "observation_anchor_mismatch",
            "The literal anchor does not match the actual observation, region and text",
        )
    if len(matches) > 1:
        raise DomainError(
            "observation_anchor_ambiguous",
            "Multiple literal anchors require an explicit anchor reference",
        )
    return matches[0] if matches else None
