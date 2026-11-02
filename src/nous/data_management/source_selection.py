"""Scoped source/object resolution and necessary relation retirement."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    AssociationBody,
    CognitiveRecord,
    CognitiveRef,
    RecordsQuery,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError

from nous.cognition.projection import evidence_in
from nous.material.named_sources import named_source_observations_in
from nous.platform.persistence import ReadView, WriteTx

if TYPE_CHECKING:
    from .sources import SourceService


async def resolve_in(
    self: SourceService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    observation_ids: tuple[str, ...],
    references: tuple[CognitiveRef, ...] = (),
    source_names: tuple[str, ...] = (),
    records: RecordsQuery | None = None,
) -> tuple[tuple[str, ...], tuple[CognitiveRecord, ...]]:
    sources = set(observation_ids)
    if source_names:
        for name in source_names:
            matched = await named_source_observations_in(view, ctx, (name,))
            if not matched:
                raise DomainError(
                    "source_not_found", "Selected source name is unavailable", source=name
                )
            sources.update(matched)
    chosen: dict[str, CognitiveRecord] = {}
    for reference in references:
        page = await self.cognition.records_in(
            view, ctx, RecordsQuery(object_ids=(reference.id,), purpose="management", limit=1)
        )
        if not page.items:
            raise DomainError("cognition_not_found", "Selected object is unavailable")
        current = page.items[0].record
        if current.reference != reference:
            raise DomainError(
                "revision_conflict", "Selected object has a newer revision", id=reference.id
            )
        chosen[current.reference.id] = current
    if records is not None:
        if records.cursor is not None or records.view != "current":
            raise DomainError(
                "source_selection_invalid",
                "Deletion selection requires current objects without a cursor",
            )
        page = await self.cognition.records_in(
            view, ctx, records
        )
        chosen.update((entry.record.reference.id, entry.record) for entry in page.items)
    if not sources and not chosen:
        raise DomainError(
            "source_selection_empty", "The selection has no authorized objects or sources"
        )
    entities = {key for key, record in chosen.items() if record.reference.kind == "entity"}
    memories = {key for key, record in chosen.items() if record.reference.kind == "memory"}
    candidates: dict[str, CognitiveRecord] = {}
    queries: list[RecordsQuery] = []
    if entities:
        queries.append(
            RecordsQuery(
                kinds=("association",),
                entity_ids=tuple(sorted(entities)),
                purpose="management",
                limit=1000,
            )
        )
    if memories:
        roots = tuple(
            sorted(
                {
                    root
                    for key, record in chosen.items()
                    if key in memories
                    for root in record.source_roots
                }
            )
        )
        queries.append(
            RecordsQuery(kinds=("association",), source_ids=roots, purpose="management", limit=1000)
        )
    for query in queries:
        page = await self.cognition.records_in(view, ctx, query)
        if page.next_cursor is not None:
            raise DomainError(
                "source_selection_too_large", "The related object selection exceeds its bound"
            )
        candidates.update((entry.record.reference.id, entry.record) for entry in page.items)
    for key, record in candidates.items():
        if (
            record.deleted
            or record.purged
            or not isinstance(record.body, AssociationBody)
            or key in chosen
        ):
            continue
        if entities.intersection((record.body.from_ref.id, record.body.to_ref.id)):
            chosen[key] = record
            continue
        supports = [
            link.reference
            for link in evidence_in(record)
            if link.polarity == "support"
            and link.reference is not None
            and link.reference.kind == "memory"
        ]
        if not any(reference.id in memories for reference in supports):
            continue
        remains = False
        for reference in supports:
            if reference.id in memories:
                continue
            try:
                remains |= await self.cognition.automatic_in(view, ctx, reference) is not None
            except DomainError as error:
                if error.code not in {"source_unavailable", "cognition_not_found"}:
                    raise
        if not remains:
            chosen[key] = record
    if len(chosen) > 1000:
        raise DomainError("source_selection_too_large", "Select at most 1000 cognitive objects")
    return tuple(sorted(sources)), tuple(chosen[key] for key in sorted(chosen))
