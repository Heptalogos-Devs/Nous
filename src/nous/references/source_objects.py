"""Read source events and evidence edges without allocating cognitive identities."""

from __future__ import annotations

from datetime import datetime
from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    CognitiveRecord,
    CognitiveRef,
    CognitiveTimes,
    MediaReference,
    TimePoint,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.records import RecordsQuery
from nous_sdk.contracts.source_objects import (
    EvidenceReference,
    EvidenceRelationRecord,
    ObservationRecord,
    ObservationReference,
)
from nous_sdk.contracts.targets import CognitiveTarget, ContextTarget
from nous_sdk.presentation import body_text
from sqlalchemy import select

from nous.material.service import observations
from nous.platform.persistence import ReadView, WriteTx

if TYPE_CHECKING:
    from .service import ReferenceDirectory


async def observation_records_in(
    self: ReferenceDirectory,
    view: ReadView | WriteTx,
    ctx: CallContext,
    observation_id: str | None = None,
) -> list[ObservationRecord]:
    result: list[ObservationRecord] = []
    statement = select(observations).where(
        observations.c.namespace_id == ctx.namespace_id,
        observations.c.active.is_(True),
    )
    if observation_id is not None:
        statement = statement.where(observations.c.namespace_id == ctx.namespace_id, observations.c.id == observation_id)
    for row in (await view.connection.execute(statement)).mappings():
        source = row["record"]
        reference = (
            MediaReference.model_validate(source["contact_source"])
            if source.get("contact_source")
            else MediaReference(kind="material", id=row["material_id"], observation_id=row["id"])
        )
        try:
            accepted = await self.materials.accepted_source_in(view, ctx, row['id'], reference)
            reference = accepted.source
            filename = accepted.material["filename"]
        except DomainError as error:
            if error.code not in {
                "source_unavailable",
                "region_not_found",
                "representation_not_found",
                "derived_region_not_found",
            }:
                raise
            continue
        recorded = TimePoint(
            coordinate=int(datetime.fromisoformat(source["recorded_at"]).timestamp() * 1000)
        )
        observed = TimePoint.model_validate(source["observed_at"])
        result.append(
            ObservationRecord(
                reference=ObservationReference(id=row["id"]),
                namespace_id=ctx.namespace_id,
                source=reference,
                event_kind=source["kind"],
                external_event_id=source.get("external_event_id"),
                caller_id=source.get("caller_id") or source.get("origin", {}).get("caller_id"),
                participants=tuple(source.get("participants", ())),
                audience=tuple(source.get("audience", ())),
                text=f"{source['kind']}: {filename}\n{observed.model_dump_json()}\n{reference.exact()}",
                source_roots=(row["id"],),
                support_roots=(row["id"],),
                times=CognitiveTimes(
                    event=TimePoint.model_validate(source["event_time"])
                    if source.get("event_time")
                    else None,
                    observation=observed,
                    formation=recorded,
                    recorded=recorded,
                ),
                commit_seq=row["commit_seq"],
            )
        )
    return result


def evidence_record(
    record: CognitiveRecord, index: int, source: MediaReference | None
) -> EvidenceRelationRecord:
    link = record.evidence[index]
    key = f"{record.reference.exact()}#{index}"
    return EvidenceRelationRecord(
        reference=EvidenceReference(id=key),
        namespace_id=record.namespace_id,
        target=record.reference,
        evidence=link,
        source=source,
        text=f"{body_text(record.body)}\n{link.polarity}: {link.role}\n{record.reference.exact()}",
        source_roots=record.source_roots,
        support_roots=record.support_roots,
        source_regions=record.source_regions,
        times=record.times,
        commit_seq=record.commit_seq,
    )


async def evidence_records_in(
    self: ReferenceDirectory, view: ReadView | WriteTx, ctx: CallContext, *, history: bool = False
) -> list[EvidenceRelationRecord]:
    result: list[EvidenceRelationRecord] = []
    command = RecordsQuery(
        view="history" if history else "current",
        purpose="exact" if history else "current",
        limit=1000,
    )
    while True:
        page = await self.cognition.records_in(view, ctx, command)
        for entry in page.items:
            record = entry.record
            for index, link in enumerate(record.evidence):
                source = None
                if link.observation_id:
                    try:
                        selected = await self.materials.accepted_source_in(view, ctx, link.observation_id, MediaReference(kind='source_region', id=link.region_id, observation_id=link.observation_id) if link.region_id else None)
                    except DomainError as error:
                        if error.code != "source_unavailable":
                            raise
                        continue
                    source = selected.source
                elif link.reference:
                    try:
                        await self.cognition.get_in(view, ctx, link.reference)
                    except DomainError as error:
                        if error.code not in {"source_unavailable", "cognition_not_found"}:
                            raise
                        continue
                result.append(evidence_record(record, index, source))
        if page.next_cursor is None:
            break
        command = command.model_copy(update={"cursor": page.next_cursor})
    return result


async def require_source_object_in(
    self: ReferenceDirectory,
    view: ReadView | WriteTx,
    ctx: CallContext,
    record: ObservationRecord | EvidenceRelationRecord,
) -> None:
    if isinstance(record, ObservationRecord):
        await self.materials.accepted_source_in(view, ctx, record.reference.id, record.source)
    else:
        target = await self.cognition.get_in(view, ctx, record.target)
        if target.purged or record.evidence not in target.evidence:
            raise DomainError("evidence_not_found", "Evidence relation is unavailable")
        if record.source:
            if record.source.kind in {"representation", "derived_region"}:
                await self.derivations.source_in(view, ctx, record.source)
            else:
                await self.materials.media_source_in(view, ctx, record.source)
        elif record.evidence.reference:
            await self.cognition.get_in(view, ctx, record.evidence.reference)


async def read_source_object_in(
    self: ReferenceDirectory,
    view: ReadView | WriteTx,
    ctx: CallContext,
    reference: ObservationReference | EvidenceReference,
) -> ObservationRecord | EvidenceRelationRecord:
    if isinstance(reference, ObservationReference):
        records = await observation_records_in(self, view, ctx, reference.id)
        if not records:
            raise DomainError("source_unavailable", "Observation is unavailable in this scope")
        return records[0]
    target, separator, ordinal = reference.id.rpartition("#")
    if not separator or not ordinal.isdecimal():
        raise DomainError(
            "invalid_reference", "Evidence reference requires an exact target and ordinal"
        )
    address, mark, revision = target.rpartition("@")
    kind, colon, identity = address.partition(":")
    if not mark or not colon or not revision.isdecimal():
        raise DomainError(
            "invalid_reference", "Evidence target must name an exact cognitive revision"
        )
    target_ref = CognitiveRef.model_validate(
        {"kind": kind, "id": identity, "revision": int(revision)}
    )
    record = await self.cognition.get_in(view, ctx, target_ref)
    index = int(ordinal)
    if record.purged or index >= len(record.evidence):
        raise DomainError("evidence_not_found", "Evidence relation is unavailable")
    link = record.evidence[index]
    source = None
    if link.observation_id:
        selected = await self.materials.accepted_source_in(view, ctx, link.observation_id, MediaReference(kind='source_region', id=link.region_id, observation_id=link.observation_id) if link.region_id else None)
        source = selected.source
    result = evidence_record(record, index, source)
    await require_source_object_in(self, view, ctx, result)
    return result


async def material_ids_in(
    self: ReferenceDirectory,
    view: ReadView | WriteTx,
    ctx: CallContext,
    targets: tuple[ContextTarget, ...],
    *,
    known_observation_ids: tuple[str, ...] = (),
) -> tuple[str, ...]:
    """Resolve custody dependencies of the current authorized projection only."""
    roots = set(known_observation_ids)
    material_ids: set[str] = set()
    for target in dict.fromkeys(targets):
        if isinstance(target, CognitiveTarget):
            record = await self.cognition.get_in(
                view, ctx, target.reference, check_visibility=False
            )
            roots.update(record.source_roots)
        else:
            material_ids.update(
                (
                    await self.materials.provenance_in(view, ctx, target.reference)
                ).material_ids
            )
    if roots:
        selected = (
            await view.connection.execute(
                select(observations.c.id, observations.c.material_id).where(
                    observations.c.namespace_id == ctx.namespace_id,
                    observations.c.id.in_(roots),
                    observations.c.active.is_(True),
                    observations.c.record["purged"].as_boolean().is_not(True),
                )
            )
        ).all()
        if {str(row.id) for row in selected} != roots:
            raise DomainError(
                "source_unavailable", "A selected context source is no longer available"
            )
        material_ids.update(str(row.material_id) for row in selected)
    return tuple(sorted(material_ids))
