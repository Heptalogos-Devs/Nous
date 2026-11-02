"""Atomic adoption of reviewed proposals against exact, authorized snapshot identities."""

from __future__ import annotations

from datetime import UTC as UTC
from datetime import datetime as datetime
from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    AdoptCognition,
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    CognitiveTimes,
    ConceptPartition,
    DomainError,
    EntityBody,
    EvidenceLink,
    MemoryBody,
    MergeConcepts,
    RetentionChange,
    SplitConcept,
)
from nous_sdk.contracts import ConceptBody as ConceptBody
from nous_sdk.contracts import JournalBody as JournalBody
from nous_sdk.contracts import RetentionState as RetentionState
from nous_sdk.contracts.cognition import CognitiveKind
from nous_sdk.contracts.growth import (
    GrowthAdoption,
    GrowthApplication,
    GrowthDraft,
    GrowthEntry,
    GrowthMemoryChange,
    GrowthMerge,
    GrowthSnapshot,
    GrowthSplit,
)
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select

from nous.cognition.evolution import EvolutionService
from nous.identity.access import require_namespace_in
from nous.material.service import observations
from nous.platform.persistence import ReadView, WriteTx

from .evolution_validation import validate_merge as validate_merge
from .evolution_validation import validate_split as validate_split
from .growth_binding import GrowthValue as GrowthValue
from .growth_binding import bind_body
from .growth_sources import evidence_in as material_evidence_in
from .lifecycle import active_body as active_body
from .lifecycle import selectable
from .lifecycle import transition as transition
from .projection import media_references_in, references_in
from .schema import revisions
from .semantics import capability_for as capability_for
from .semantics import validate_body as validate_body

if TYPE_CHECKING:
    from .records import CognitiveRecords
if TYPE_CHECKING:
    from .records import CognitiveRecords


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


class GrowthService:
    def __init__(self, records: CognitiveRecords, *, evolution: EvolutionService):
        self.evolution = evolution
        self.records = records

    async def validate_growth_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        draft: GrowthDraft,
        snapshot: GrowthSnapshot,
    ) -> None:
        actual = await self.catalog_in(view, ctx, snapshot)
        source_evidence = await material_evidence_in(self.records, view, ctx, snapshot)
        catalog: dict[str, CognitiveRecord | GrowthValue] = dict(actual)
        snapshot_keys = frozenset(actual)
        check_keys(draft, snapshot_keys | frozenset(source_evidence))
        checked: set[str] = set()

        def lookup(key: str) -> CognitiveRecord | GrowthValue:
            if key not in catalog:
                raise DomainError(
                    "growth_key_unknown",
                    "A proposal local key is unknown or used before its declaration",
                    key=key,
                )
            return catalog[key]

        async def target(key: str) -> CognitiveRecord | GrowthValue:
            record = lookup(key)
            if key in snapshot_keys and record.reference.id not in checked:
                original = actual[key]
                current = await view.connection.scalar(
                    select(revisions.c.revision).where(
                        revisions.c.id == original.reference.id,
                        revisions.c.namespace_id == ctx.namespace_id,
                        revisions.c.head.is_(True),
                    )
                )
                if current != original.reference.revision:
                    raise DomainError(
                        "growth_revision_conflict",
                        "A group target changed after its frozen snapshot",
                    )
                checked.add(original.reference.id)
            return record

        def planned(
            key: str, body: object, previous: CognitiveRecord | GrowthValue | None = None
        ) -> GrowthValue:
            from nous_sdk.contracts import CognitiveBody
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
                times=previous.times if previous else None,
            )

        def publish(key: str, value: GrowthValue) -> None:
            for alias, existing in tuple(catalog.items()):
                if existing.reference.id == value.reference.id:
                    catalog[alias] = value
            catalog[key] = value

        for action in draft.actions:
            if isinstance(action, GrowthAdoption):
                previous = await target(action.target_key) if action.target_key else None
                for key in (
                    *action.evidence_keys,
                    *(binding.key for binding in action.bindings if binding.kind == "cognitive"),
                ):
                    if key not in source_evidence:
                        lookup(key)
                if previous is not None and (
                    previous.deleted or not active_body(previous.body, previous.lifecycle)
                ):
                    raise DomainError(
                        "growth_cognition_inactive", "Growth cannot revise inactive cognition"
                    )
                body = bind_body(action, catalog, snapshot.watermark, snapshot.sources)
                if body.kind == "episode" and (previous is None or previous.body != body):
                    await require_namespace_in(view, ctx, "episodes")
                await require_namespace_in(view, ctx, capability_for(body))
                if previous is not None and body.kind != previous.reference.kind:
                    raise DomainError(
                        "cognitive_kind_immutable",
                        "A cognitive identity cannot change its domain kind",
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
                publish(action.key, planned(action.key, body, previous))
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
                publish(
                    action.key,
                    planned(
                        action.key,
                        original.body.model_copy(update={"members": tuple(members.values())}),
                        original,
                    ),
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
                publish(
                    action.key,
                    planned(
                        action.key, original.body.model_copy(update={"state": "split"}), original
                    ),
                )
            else:
                original = await target(action.target_key)
                if (
                    original.reference.kind not in {"memory", "association"}
                    or original.body is None
                    or original.purged
                ):
                    raise DomainError(
                        "retention_unavailable",
                        "Retention requires a nonpurged Memory or Association",
                    )
                if isinstance(action, GrowthMemoryChange) and original.reference.kind != "memory":
                    raise DomainError(
                        "memory_required", "The public memory action requires a Memory target"
                    )
                if original.deleted and action.transition != "restore":
                    raise DomainError(
                        "memory_deleted", "Restore Memory before changing its lifecycle"
                    )
                change = RetentionChange(
                    reference=original.reference,
                    action=action.transition,
                    strength=action.strength,
                    hours=action.hours,
                    reason=action.reason,
                )
                now = int(datetime.now(UTC).timestamp() * 1000)
                lifecycle = original.lifecycle or RetentionState(
                    anchor_unix_ms=original.times.recorded.coordinate
                    if original.times
                    else ctx.clock.coordinate
                )
                updated = transition(lifecycle, change, now)
                value = planned(action.key, original.body, original)
                publish(
                    action.key,
                    GrowthValue(
                        value.reference,
                        value.body,
                        value.commit_seq,
                        updated,
                        False if action.transition == "restore" else original.deleted,
                        times=original.times,
                    ),
                )

    async def growth_snapshot_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        kinds: tuple[CognitiveKind, ...] | None = None,
        limit: int = 500,
    ) -> GrowthSnapshot:
        if not 1 <= limit <= 1000:
            raise DomainError("growth_limit", "Growth snapshots require a limit between 1 and 1000")
        subject = await require_namespace_in(view, ctx)
        statement = (
            select(revisions.c.record)
            .where(revisions.c.namespace_id == ctx.namespace_id, revisions.c.head.is_(True))
            .order_by(revisions.c.commit_seq.desc(), revisions.c.id)
        )
        if kinds is not None:
            statement = statement.where(revisions.c.kind.in_(kinds))
        entries: list[GrowthEntry] = []
        roots: set[str] = set()
        for value in (await view.connection.execute(statement)).scalars():
            record = await self.records.project_record_in(
                view, ctx, CognitiveRecord.model_validate(value)
            )
            if not selectable(record) or not await self.records.visible_in(view, ctx, record):
                continue
            entries.append(GrowthEntry(key=f"c{len(entries)}", record=record))
            roots.update(record.source_roots)
            if len(entries) == limit:
                break
        material_ids = tuple(
            sorted(
                set(
                    (
                        await view.connection.execute(
                            select(observations.c.material_id).where(
                                observations.c.id.in_(roots),
                                observations.c.namespace_id == ctx.namespace_id,
                            )
                        )
                    ).scalars()
                )
            )
        )
        all_materials = set(material_ids)
        internal = ctx.model_copy(update={"allowed_source_ids": None, "source_constraints": ()})
        for entry in entries:
            for media in media_references_in(entry.record.body):
                if media.kind in {"representation", "derived_region"}:
                    all_materials.update(
                        (
                            await self.records.derivations.source_in(view, internal, media)
                        ).material_ids
                    )
                else:
                    all_materials.update(
                        (
                            await self.records.materials.provenance_in(
                                view, internal, media, derivations=self.records.derivations
                            )
                        ).material_ids
                    )
        return GrowthSnapshot(
            watermark=view.commit_seq,
            entries=tuple(entries),
            material_ids=tuple(sorted(all_materials)),
            capabilities=tuple(str(value) for value in subject.enabled_features),
        )

    async def apply_growth_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        draft: GrowthDraft,
        snapshot: GrowthSnapshot,
        *,
        producer_call_id: str | None = None,
    ) -> GrowthApplication:
        await self.validate_growth_in(tx, ctx, draft, snapshot)
        source_evidence = await material_evidence_in(self.records, tx, ctx, snapshot)
        if snapshot.watermark > tx.commit_seq:
            raise DomainError(
                "growth_snapshot_future",
                "A growth snapshot cannot come from a later authority state",
            )
        catalog: dict[str, CognitiveRecord] = {}
        for entry in snapshot.entries:
            if entry.key in catalog or entry.record.commit_seq > snapshot.watermark:
                raise DomainError(
                    "growth_snapshot_invalid",
                    "Snapshot keys and knowledge cutoff must be consistent",
                )
            actual = await self.records.get_in(tx, ctx, entry.record.reference)
            if actual.commit_seq > snapshot.watermark:
                raise DomainError(
                    "growth_snapshot_invalid",
                    "A snapshot cannot include knowledge recorded after its cutoff",
                )
            catalog[entry.key] = actual
        snapshot_keys = frozenset(catalog)
        keys = [
            key
            for action in draft.actions
            for key in (
                (action.key, *(part.key for part in action.partitions))
                if isinstance(action, GrowthSplit)
                else (action.key,)
            )
        ]
        if len(keys) != len(set(keys)) or set(keys).intersection(
            snapshot_keys | frozenset(source_evidence)
        ):
            raise DomainError(
                "growth_key_conflict",
                "Proposal local keys must be unique and cannot shadow snapshot keys",
            )
        results: list[CognitiveRecord] = []

        def publish(key: str | None, record: CognitiveRecord) -> None:
            for alias, existing in tuple(catalog.items()):
                if existing.reference.id == record.reference.id:
                    catalog[alias] = record
            if key is not None:
                catalog[key] = record

        for action in draft.actions:
            if not isinstance(action, GrowthAdoption):
                if isinstance(action, GrowthMerge):
                    required_keys = (action.target_key, *action.source_keys)
                elif isinstance(action, GrowthSplit):
                    required_keys = (
                        action.source_key,
                        *(key for part in action.partitions for key in part.member_keys),
                    )
                else:
                    required_keys = (action.target_key,)
                if any(key not in catalog for key in required_keys):
                    raise DomainError(
                        "growth_key_unknown",
                        "Structural actions require actual snapshot or earlier action keys",
                    )
                if isinstance(action, GrowthMerge):
                    changed = await self.evolution.merge_concepts_in(
                        tx,
                        ctx,
                        MergeConcepts(
                            target=catalog[action.target_key].reference,
                            sources=tuple(catalog[key].reference for key in action.source_keys),
                            reason=action.reason,
                        ),
                    )
                    for changed_record in changed:
                        publish(
                            action.key if changed_record == changed[0] else None, changed_record
                        )
                elif isinstance(action, GrowthSplit):
                    changed = await self.evolution.split_concept_in(
                        tx,
                        ctx,
                        SplitConcept(
                            source=catalog[action.source_key].reference,
                            partitions=tuple(
                                ConceptPartition(
                                    label=part.label,
                                    members=tuple(
                                        catalog[key].reference for key in part.member_keys
                                    ),
                                )
                                for part in action.partitions
                            ),
                            reason=action.reason,
                        ),
                    )
                    for part, record in zip(action.partitions, changed[:-1], strict=True):
                        catalog[part.key] = record
                    publish(action.key, changed[-1])
                else:
                    if (
                        isinstance(action, GrowthMemoryChange)
                        and catalog[action.target_key].reference.kind != "memory"
                    ):
                        raise DomainError(
                            "memory_required", "The public memory action requires a Memory target"
                        )
                    changed = (
                        await self.records.change_retention_in(
                            tx,
                            ctx,
                            RetentionChange(
                                reference=catalog[action.target_key].reference,
                                action=action.transition,
                                strength=action.strength,
                                hours=action.hours,
                                reason=action.reason,
                            ),
                        ),
                    )
                    publish(action.key, changed[0])
                results.extend(changed)
                continue
            required = {
                *action.evidence_keys,
                *(binding.key for binding in action.bindings if binding.kind == "cognitive"),
            }
            if action.target_key is not None:
                if action.target_key not in catalog:
                    raise DomainError(
                        "growth_target_unknown",
                        "Revision targets must identify an actual snapshot key",
                    )
                required.add(action.target_key)
            unknown = required.difference(catalog).difference(source_evidence)
            if unknown:
                raise DomainError(
                    "growth_key_unknown",
                    "Proposal references an unknown or later local key",
                    unknown_keys=TypeAdapter(list[JsonValue]).validate_python(sorted(unknown)),
                )
            body = bind_body(action, catalog, snapshot.watermark, snapshot.sources)
            target = catalog[action.target_key] if action.target_key else None
            if target is not None:
                if not selectable(target):
                    raise DomainError(
                        "growth_memory_inactive",
                        "Automatic growth cannot revise inactive cognition",
                    )
            if (
                target is not None
                and isinstance(target.body, MemoryBody)
                and isinstance(body, MemoryBody)
            ):
                body = body.model_copy(update={"media": target.body.media})
            if (
                target is not None
                and isinstance(target.body, EntityBody)
                and isinstance(body, EntityBody)
            ):
                body = body.model_copy(update={"profile_controls": target.body.profile_controls})
            links = tuple(
                link.model_copy(update={"role": action.reason})
                for key in action.evidence_keys
                for link in (
                    source_evidence[key]
                    if key in source_evidence
                    else (EvidenceLink(reference=catalog[key].reference),)
                )
            )
            adopted = await self.records.adopt_in(
                tx,
                ctx,
                AdoptCognition(
                    id=target.reference.id if target else None,
                    expected_revision=target.reference.revision if target else 0,
                    body=body,
                    evidence=links,
                    producer_call_id=producer_call_id,
                    times=CognitiveTimes(formation=ctx.clock, recorded=ctx.clock),
                ),
            )
            publish(action.key, adopted)
            results.append(adopted)
        changed_ids = {record.reference.id for record in results}
        return GrowthApplication(
            records=tuple(results),
            bindings={
                key: value.reference
                for key, value in catalog.items()
                if value.reference.id in changed_ids
            },
        )

    async def catalog_in(
        self, view: ReadView | WriteTx, ctx: CallContext, snapshot: GrowthSnapshot
    ) -> dict[str, CognitiveRecord]:
        if snapshot.watermark > view.commit_seq:
            raise DomainError(
                "growth_snapshot_future", "Snapshot is newer than the current authority view"
            )
        catalog: dict[str, CognitiveRecord] = {}
        for entry in snapshot.entries:
            record = await self.records.get_in(view, ctx, entry.record.reference)
            if entry.key in catalog or record.commit_seq > snapshot.watermark:
                raise DomainError(
                    "growth_snapshot_invalid", "Snapshot keys and knowledge cutoff are inconsistent"
                )
            if not selectable(record):
                raise DomainError(
                    "growth_snapshot_inactive",
                    "Automatic growth requires active snapshot cognition",
                )
            catalog[entry.key] = record
        return catalog
