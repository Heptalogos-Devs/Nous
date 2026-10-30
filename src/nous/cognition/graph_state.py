"""The graph directory is a view of authorized canonical revisions."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.contracts import (
    CallContext,
    CognitiveRecord,
    DomainError,
    EntityBody,
    MemoryBody,
)
from nous_wave.contracts.graph_management import GraphSnapshot
from sqlalchemy import select

from nous.identity.service import require_subject_in
from nous.platform.persistence import ReadView, WriteTx

from .lifecycle import selectable
from .projection import references_in
from .schema import revisions

if TYPE_CHECKING:
    from .service import CognitionService


async def heads_in(
    view: ReadView | WriteTx,
    ctx: CallContext,
    kinds: tuple[str, ...] = ("entity", "association"),
    *,
    reference_ids: tuple[str, ...] | None = None,
) -> tuple[CognitiveRecord, ...]:
    statement = select(revisions.c.record).where(
        revisions.c.namespace_id == ctx.namespace_id,
        revisions.c.subject_id == ctx.require_subject(),
        revisions.c.head.is_(True),
        revisions.c.kind.in_(kinds),
    )
    if reference_ids is not None:
        statement = statement.where(revisions.c.id.in_(reference_ids))
    values = (await view.connection.execute(statement.order_by(revisions.c.id))).scalars()
    return tuple(CognitiveRecord.model_validate(value) for value in values)


def resolve_entity(records: tuple[CognitiveRecord, ...], token: str) -> CognitiveRecord | None:
    key = token.strip().casefold()
    if not key:
        return None
    matches = [
        record
        for record in records
        if isinstance(record.body, EntityBody)
        and not record.purged
        and (
            token
            in {record.reference.id, record.reference.exact(), *record.body.identifiers.values()}
            or key
            in {value.casefold() for value in (record.body.label, *record.body.effective_aliases)}
        )
    ]
    if len(matches) > 1:
        raise DomainError("ambiguous_entity", "The selector identifies multiple Entity identities")
    return matches[0] if matches else None


async def graph_snapshot(self: CognitionService, ctx: CallContext) -> GraphSnapshot:
    async with self.authority.read(ctx) as view:
        await require_subject_in(view, ctx)
        records: list[CognitiveRecord] = []
        paragraphs: dict[str, CognitiveRecord] = {}
        for record in await heads_in(view, ctx):
            if not selectable(record) or not await self.visible_in(view, ctx, record):
                continue
            records.append(record)
            references = tuple(references_in(record.body)) + tuple(
                link.reference for link in record.evidence if link.reference
            )
            for reference in references:
                if reference.kind != "memory" or reference.exact() in paragraphs:
                    continue
                try:
                    value = await self.get_in(view, ctx, reference)
                except DomainError as error:
                    if error.code in {"source_unavailable", "cognition_not_found"}:
                        continue
                    raise
                if selectable(value) and isinstance(value.body, MemoryBody):
                    paragraphs[reference.exact()] = value
        return GraphSnapshot(
            records=tuple(records), paragraphs=tuple(paragraphs.values()), watermark=view.commit_seq
        )
