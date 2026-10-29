"""Read-only review validation reuses the exact body binder and domain invariants."""

from __future__ import annotations

from datetime import UTC, datetime
from typing import TYPE_CHECKING

from nous_wave.contracts import (
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    ConceptBody,
    ConceptPartition,
    DomainError,
    JournalBody,
    MemoryBody,
    MemoryChange,
    MemoryLifecycle,
)
from nous_wave.contracts.growth import (
    GrowthAdoption,
    GrowthDraft,
    GrowthMerge,
    GrowthSnapshot,
    GrowthSplit,
)
from sqlalchemy import select

from nous.identity.service import require_subject_in
from nous.platform.persistence import ReadView, WriteTx

from .evolution_validation import validate_merge, validate_split
from .growth_binding import GrowthValue, bind_body
from .lifecycle import transition
from .projection import references_in
from .schema import revisions
from .semantics import capability_for, validate_body

if TYPE_CHECKING:
    from .service import CognitionService


async def catalog_in(
    self: CognitionService, view: ReadView | WriteTx, ctx: CallContext, snapshot: GrowthSnapshot
) -> dict[str, CognitiveRecord]:
    if snapshot.watermark > view.commit_seq:
        raise DomainError(
            "growth_snapshot_future", "Snapshot is newer than the current authority view"
        )
    catalog: dict[str, CognitiveRecord] = {}
    for entry in snapshot.entries:
        record = await self.get_in(view, ctx, entry.record.reference)
        if entry.key in catalog or record.commit_seq > snapshot.watermark:
            raise DomainError(
                "growth_snapshot_invalid", "Snapshot keys and knowledge cutoff are inconsistent"
            )
        catalog[entry.key] = record
    return catalog


def check_keys(draft: GrowthDraft, snapshot_keys: frozenset[str]) -> None:
    keys = [
        key
        for action in draft.actions
        for key in (
            (action.key, *(part.key for part in action.partitions))
            if isinstance(action, GrowthSplit)
            else (action.key,)
        )
    ]
    if len(keys) != len(set(keys)) or set(keys).intersection(snapshot_keys):
        raise DomainError(
            "growth_key_conflict", "Proposal keys must be unique and cannot shadow snapshot keys"
        )


async def validate_growth_in(
    self: CognitionService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    draft: GrowthDraft,
    snapshot: GrowthSnapshot,
) -> None:
    actual = await catalog_in(self, view, ctx, snapshot)
    catalog: dict[str, CognitiveRecord | GrowthValue] = dict(actual)
    snapshot_keys = frozenset(actual)
    check_keys(draft, snapshot_keys)
    revised: set[str] = set()

    def lookup(key: str) -> CognitiveRecord | GrowthValue:
        if key not in catalog:
            raise DomainError(
                "growth_key_unknown",
                "A proposal local key is unknown or used before its declaration",
                key=key,
            )
        return catalog[key]

    async def target(key: str) -> CognitiveRecord:
        if key not in snapshot_keys:
            raise DomainError(
                "growth_target_unknown", "Revision targets must identify a supplied snapshot key"
            )
        record = actual[key]
        current = await view.connection.scalar(
            select(revisions.c.revision).where(
                revisions.c.id == record.reference.id,
                revisions.c.namespace_id == ctx.namespace_id,
                revisions.c.subject_id == ctx.subject_id,
                revisions.c.head.is_(True),
            )
        )
        if current != record.reference.revision or record.reference.id in revised:
            raise DomainError(
                "growth_revision_conflict", "A target is stale or already revised by this proposal"
            )
        revised.add(record.reference.id)
        return record

    def planned(key: str, body: object, previous: CognitiveRecord | None = None) -> GrowthValue:
        from nous_wave.contracts import CognitiveBody
        from pydantic import TypeAdapter

        content = TypeAdapter(CognitiveBody).validate_python(body)
        reference = CognitiveRef(
            kind=content.kind,
            id=previous.reference.id if previous else f"proposal/{key}",
            revision=previous.reference.revision + 1 if previous else 1,
        )
        return GrowthValue(
            reference,
            content,
            snapshot.watermark + 1,
            previous.lifecycle if previous else None,
            previous.deleted if previous else False,
        )

    for action in draft.actions:
        if isinstance(action, GrowthAdoption):
            previous = await target(action.target_key) if action.target_key else None
            for key in (*action.evidence_keys, *(binding.key for binding in action.bindings)):
                lookup(key)
            if (
                previous is not None
                and previous.lifecycle
                and previous.lifecycle.activation != "active"
            ):
                raise DomainError(
                    "growth_memory_inactive", "Growth cannot revise frozen or retired Memory"
                )
            body = bind_body(action, catalog, snapshot.watermark)
            await require_subject_in(view, ctx, capability_for(body))
            if previous is not None and body.kind != previous.reference.kind:
                raise DomainError(
                    "cognitive_kind_immutable", "A cognitive identity cannot change its domain kind"
                )
            related = {record.reference.exact(): record.body for record in catalog.values()}
            validate_body(
                body, previous.reference.id if previous else f"proposal/{action.key}", related
            )
            if isinstance(body, JournalBody) and any(
                record.commit_seq > snapshot.watermark
                for record in catalog.values()
                if record.reference.exact()
                in {reference.exact() for reference in references_in(body)}
            ):
                raise DomainError(
                    "knowledge_cutoff",
                    "Journal cannot cite cognition formed after its snapshot perspective",
                )
            catalog[action.key] = planned(action.key, body, previous)
        elif isinstance(action, GrowthMerge):
            records = [await target(key) for key in (action.target_key, *action.source_keys)]
            validate_merge(records)
            original = records[0]
            assert isinstance(original.body, ConceptBody)
            members = {
                ref.exact(): ref
                for record in records
                if isinstance(record.body, ConceptBody)
                for ref in record.body.members
            }
            catalog[action.key] = planned(
                action.key,
                original.body.model_copy(update={"members": tuple(members.values())}),
                original,
            )
        elif isinstance(action, GrowthSplit):
            original = await target(action.source_key)
            partitions = tuple(
                ConceptPartition(
                    label=part.label,
                    members=tuple(lookup(key).reference for key in part.member_keys),
                )
                for part in action.partitions
            )
            validate_split(original, partitions)
            assert isinstance(original.body, ConceptBody)
            for part, specification in zip(action.partitions, partitions, strict=True):
                catalog[part.key] = planned(
                    part.key,
                    ConceptBody(
                        label=part.label,
                        description=original.body.description,
                        members=specification.members,
                    ),
                )
            catalog[action.key] = planned(
                action.key, original.body.model_copy(update={"state": "split"}), original
            )
        else:
            original = await target(action.target_key)
            if not isinstance(original.body, MemoryBody) or original.purged:
                raise DomainError(
                    "memory_unavailable", "Maintenance requires an actual nonpurged Memory"
                )
            if original.deleted and action.transition != "restore":
                raise DomainError("memory_deleted", "Restore Memory before changing its lifecycle")
            change = MemoryChange(
                reference=original.reference,
                action=action.transition,
                strength=action.strength,
                hours=action.hours,
                reason=action.reason,
            )
            now = int(datetime.now(UTC).timestamp() * 1000)
            lifecycle = original.lifecycle or MemoryLifecycle(
                anchor_unix_ms=original.times.recorded.coordinate
            )
            updated = transition(lifecycle, change, now)
            value = planned(action.key, original.body, original)
            catalog[action.key] = GrowthValue(
                value.reference,
                value.body,
                value.commit_seq,
                updated,
                False if action.transition == "restore" else original.deleted,
            )
