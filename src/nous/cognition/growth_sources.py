"""Material evidence remains material until a reviewed proposal adopts cognition."""

from __future__ import annotations

from datetime import datetime
from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    CallContext,
    CognitiveTimes,
    DomainError,
    EvidenceLink,
    ImportUnit,
    MediaReference,
    TimePoint,
)
from nous_sdk.contracts.derivation import StructuredRepresentation, TextRepresentation
from nous_sdk.contracts.growth import GrowthSnapshot, GrowthSource

from nous.material.service import require_observation_in
from nous.platform.persistence import ReadView, WriteTx
from nous.platform.persistence.authority import canonical_json

if TYPE_CHECKING:
    from .records import CognitiveRecords


async def evidence_in(
    self: CognitiveRecords, view: ReadView | WriteTx, ctx: CallContext, snapshot: GrowthSnapshot
) -> dict[str, tuple[EvidenceLink, ...]]:
    keys = {entry.key for entry in snapshot.entries}
    result: dict[str, tuple[EvidenceLink, ...]] = {}
    for source in snapshot.sources:
        if source.key in keys or source.key in result:
            raise DomainError("growth_key_conflict", "Material and cognitive keys must be distinct")
        provenance = await self.materials.provenance_in(
            view, ctx, source.source, derivations=self.derivations
        )
        for link in source.evidence:
            if link.observation_id is None or link.observation_id not in provenance.roots:
                raise DomainError(
                    "growth_source_invalid", "Material evidence differs from its actual provenance"
                )
            await require_observation_in(view, ctx, link.observation_id, region_id=link.region_id)
        result[source.key] = source.evidence
    return result


async def from_units_in(
    self: CognitiveRecords,
    view: ReadView | WriteTx,
    ctx: CallContext,
    units: tuple[ImportUnit, ...],
) -> tuple[GrowthSource, ...]:
    values: list[GrowthSource] = []
    for index, unit in enumerate(units):
        assert unit.observation_id is not None
        event = await require_observation_in(
            view, ctx, unit.observation_id, region_id=unit.source_region_id
        )
        source = unit.source or MediaReference(
            kind="source_region" if unit.source_region_id else "material",
            id=unit.source_region_id or unit.material_id,
            observation_id=unit.observation_id,
        )
        values.append(
            GrowthSource(
                key=f"s{index}",
                source=source,
                evidence=(
                    EvidenceLink(
                        observation_id=unit.observation_id, region_id=unit.source_region_id
                    ),
                ),
                times=CognitiveTimes(
                    event=TimePoint.model_validate(event["record"]["document"]["event_time"])
                    if event["record"].get("document", {}).get("event_time") is not None
                    else None,
                    formation=ctx.clock,
                    recorded=TimePoint(
                        coordinate=int(
                            datetime.fromisoformat(event["record"]["recorded_at"]).timestamp()
                            * 1000
                        )
                    ),
                    observation=TimePoint.model_validate(event["record"]["observed_at"]),
                ),
                requires_media=unit.strategy == "media",
            )
        )
    return tuple(values)


async def from_references_in(
    self: CognitiveRecords,
    view: ReadView | WriteTx,
    ctx: CallContext,
    references: tuple[MediaReference, ...],
    *,
    start: int = 0,
) -> tuple[GrowthSource, ...]:
    values: list[GrowthSource] = []
    for index, reference in enumerate(references, start):
        provenance = await self.materials.provenance_in(
            view, ctx, reference, derivations=self.derivations
        )
        event_id = reference.observation_id or (
            provenance.roots[0] if len(provenance.roots) == 1 else None
        )
        observation = recorded = None
        if event_id is not None:
            event = await require_observation_in(view, ctx, event_id)
            observation = TimePoint.model_validate(event["record"]["observed_at"])
            recorded = TimePoint(
                coordinate=int(
                    datetime.fromisoformat(event["record"]["recorded_at"]).timestamp() * 1000
                )
            )
        values.append(
            GrowthSource(
                key=f"s{index}",
                source=reference,
                evidence=tuple(EvidenceLink(observation_id=root) for root in provenance.roots),
                times=CognitiveTimes(
                    observation=observation, formation=ctx.clock, recorded=recorded or ctx.clock
                ),
            )
        )
    return tuple(values)


async def read_in(
    self: CognitiveRecords,
    view: ReadView | WriteTx,
    ctx: CallContext,
    sources: tuple[GrowthSource, ...],
) -> tuple[GrowthSource, ...]:
    values: list[GrowthSource] = []
    for source in sources:
        text = ""
        if source.source.kind in {"representation", "derived_region"}:
            selected = await self.derivations.selected_in(view, ctx, source.source)
            if isinstance(selected.payload, TextRepresentation):
                text = selected.payload.text
            elif isinstance(selected.payload, StructuredRepresentation):
                text = canonical_json(selected.payload.value)
        else:
            material, region, _ = await self.materials.media_source_in(view, ctx, source.source)
            if material["text"] is not None:
                from nous.material.text import source_region_text

                text = (
                    source_region_text(material["text"], region)
                    if region is not None
                    else material["text"]
                )
        values.append(
            source.model_copy(
                update={"text": text, "requires_media": source.requires_media or not text}
            )
        )
    return tuple(values)
