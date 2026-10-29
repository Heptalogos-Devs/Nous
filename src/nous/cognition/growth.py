"""Atomic adoption of reviewed proposals against exact, authorized snapshot identities."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.contracts import (
    AdoptCognition,
    CallContext,
    CognitiveRecord,
    CognitiveTimes,
    ConceptPartition,
    DomainError,
    EvidenceLink,
    MemoryBody,
    MemoryChange,
    MergeConcepts,
    SplitConcept,
)
from nous_wave.contracts.cognition import CognitiveKind
from nous_wave.contracts.growth import (
    GrowthAdoption,
    GrowthDraft,
    GrowthEntry,
    GrowthMerge,
    GrowthSnapshot,
    GrowthSplit,
)
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select

from nous.identity.service import require_subject_in
from nous.material.service import observations
from nous.platform.persistence import ReadView, WriteTx

from .growth_binding import bind_body
from .projection import media_references_in
from .schema import revisions

if TYPE_CHECKING:
    from .service import CognitionService


async def growth_snapshot_in(
    self: CognitionService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    kinds: tuple[CognitiveKind, ...] | None = None,
    limit: int = 500,
) -> GrowthSnapshot:
    if not 1 <= limit <= 1000:
        raise DomainError("growth_limit", "Growth snapshots require a limit between 1 and 1000")
    subject = await require_subject_in(view, ctx)
    statement = (
        select(revisions.c.record)
        .where(
            revisions.c.namespace_id == ctx.namespace_id,
            revisions.c.subject_id == ctx.subject_id,
            revisions.c.head.is_(True),
        )
        .order_by(revisions.c.commit_seq.desc(), revisions.c.id)
    )
    if kinds is not None:
        statement = statement.where(revisions.c.kind.in_(kinds))
    entries: list[GrowthEntry] = []
    roots: set[str] = set()
    for value in (await view.connection.execute(statement)).scalars():
        record = CognitiveRecord.model_validate(value)
        if record.deleted or record.purged or not await self.visible_in(view, ctx, record):
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
                            observations.c.subject_id == ctx.subject_id,
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
                    (await self.derivations.source_in(view, internal, media)).material_ids
                )
            else:
                all_materials.update(
                    (
                        await self.materials.provenance_in(
                            view, internal, media, derivations=self.derivations
                        )
                    ).material_ids
                )
    return GrowthSnapshot(
        watermark=view.commit_seq,
        entries=tuple(entries),
        material_ids=tuple(sorted(all_materials)),
        capabilities=tuple(str(value) for value in subject["capabilities"]),
    )


async def apply_growth_in(
    self: CognitionService,
    tx: WriteTx,
    ctx: CallContext,
    draft: GrowthDraft,
    snapshot: GrowthSnapshot,
    *,
    producer_call_id: str | None = None,
) -> tuple[CognitiveRecord, ...]:
    await self.validate_growth_in(tx, ctx, draft, snapshot)
    if snapshot.watermark > tx.commit_seq:
        raise DomainError(
            "growth_snapshot_future", "A growth snapshot cannot come from a later authority state"
        )
    catalog: dict[str, CognitiveRecord] = {}
    for entry in snapshot.entries:
        if entry.key in catalog or entry.record.commit_seq > snapshot.watermark:
            raise DomainError(
                "growth_snapshot_invalid", "Snapshot keys and knowledge cutoff must be consistent"
            )
        # Re-read the authority; stored proposal bodies never become replacement authority.
        actual = await self.get_in(tx, ctx, entry.record.reference)
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
    if len(keys) != len(set(keys)) or set(keys).intersection(snapshot_keys):
        raise DomainError(
            "growth_key_conflict",
            "Proposal local keys must be unique and cannot shadow snapshot keys",
        )
    results: list[CognitiveRecord] = []
    revised: set[str] = set()
    for action in draft.actions:
        if not isinstance(action, GrowthAdoption):
            if isinstance(action, GrowthMerge):
                required_keys = (action.target_key, *action.source_keys)
                target_keys = required_keys
            elif isinstance(action, GrowthSplit):
                required_keys = (
                    action.source_key,
                    *(key for part in action.partitions for key in part.member_keys),
                )
                target_keys = (action.source_key,)
            else:
                required_keys = (action.target_key,)
                target_keys = required_keys
            if any(key not in catalog for key in required_keys):
                raise DomainError(
                    "growth_key_unknown",
                    "Structural actions require actual snapshot or earlier action keys",
                )
            if any(
                key not in snapshot_keys or catalog[key].reference.id in revised
                for key in target_keys
            ):
                raise DomainError(
                    "growth_revision_conflict",
                    "Each structural target must be one unmodified snapshot identity",
                )
            revised.update(catalog[key].reference.id for key in target_keys)
            if isinstance(action, GrowthMerge):
                changed = await self.merge_concepts_in(
                    tx,
                    ctx,
                    MergeConcepts(
                        target=catalog[action.target_key].reference,
                        sources=tuple(catalog[key].reference for key in action.source_keys),
                        reason=action.reason,
                    ),
                )
                catalog[action.key] = changed[0]
            elif isinstance(action, GrowthSplit):
                changed = await self.split_concept_in(
                    tx,
                    ctx,
                    SplitConcept(
                        source=catalog[action.source_key].reference,
                        partitions=tuple(
                            ConceptPartition(
                                label=part.label,
                                members=tuple(catalog[key].reference for key in part.member_keys),
                            )
                            for part in action.partitions
                        ),
                        reason=action.reason,
                    ),
                )
                for part, record in zip(action.partitions, changed[:-1], strict=True):
                    catalog[part.key] = record
                catalog[action.key] = changed[-1]
            else:
                changed = (
                    await self.change_memory_in(
                        tx,
                        ctx,
                        MemoryChange(
                            reference=catalog[action.target_key].reference,
                            action=action.transition,
                            strength=action.strength,
                            hours=action.hours,
                            reason=action.reason,
                        ),
                    ),
                )
                catalog[action.key] = changed[0]
            results.extend(changed)
            continue
        required = {*action.evidence_keys, *(binding.key for binding in action.bindings)}
        if action.target_key is not None:
            if action.target_key not in snapshot_keys:
                raise DomainError(
                    "growth_target_unknown", "Revision targets must identify an actual snapshot key"
                )
            required.add(action.target_key)
        unknown = required.difference(catalog)
        if unknown:
            raise DomainError(
                "growth_key_unknown",
                "Proposal references an unknown or later local key",
                unknown_keys=TypeAdapter(list[JsonValue]).validate_python(sorted(unknown)),
            )
        body = bind_body(action, catalog, snapshot.watermark)
        target = catalog[action.target_key] if action.target_key else None
        if target is not None:
            if target.reference.id in revised:
                raise DomainError(
                    "growth_revision_conflict", "A proposal can revise each snapshot identity once"
                )
            revised.add(target.reference.id)
            if target.lifecycle is not None and target.lifecycle.activation != "active":
                raise DomainError(
                    "growth_memory_inactive",
                    "Automatic growth cannot revise frozen or retired Memory",
                )
        if (
            target is not None
            and isinstance(target.body, MemoryBody)
            and isinstance(body, MemoryBody)
        ):
            body = body.model_copy(update={"media": target.body.media})
        links = tuple(
            EvidenceLink(reference=catalog[key].reference, role=action.reason)
            for key in action.evidence_keys
        )
        adopted = await self.adopt_in(
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
        catalog[action.key] = adopted
        results.append(adopted)
    return tuple(results)
