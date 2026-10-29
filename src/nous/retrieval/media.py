"""Authorized original and region projections for the shared retrieval planner."""

from __future__ import annotations

import asyncio
from datetime import datetime
from typing import TYPE_CHECKING

from nous_wave.contracts import (
    CallContext,
    CognitiveTimes,
    DomainError,
    MediaEmbeddingInput,
    MediaRecord,
    MediaReference,
    ReferenceAddresses,
    SourceRegion,
    TimePoint,
)
from sqlalchemy import select

from nous.material.service import materials, observations, regions, source_permitted
from nous.material.text import source_region_text
from nous.platform.content import ContentObject
from nous.platform.persistence import ReadView, WriteTx

if TYPE_CHECKING:
    from nous.material.derived import DerivedService
    from nous.material.service import MaterialService


async def media_records_in(
    view: ReadView | WriteTx,
    ctx: CallContext,
    material_service: MaterialService | None = None,
    derivations: DerivedService | None = None,
    *,
    include_text: bool = True,
) -> list[MediaRecord]:
    sources = (
        (
            await view.connection.execute(
                select(observations).where(
                    observations.c.namespace_id == ctx.namespace_id,
                    observations.c.subject_id == ctx.subject_id,
                    observations.c.active.is_(True),
                )
            )
        )
        .mappings()
        .all()
    )
    material_rows = {
        row["id"]: row["record"]
        for row in (
            await view.connection.execute(
                select(materials).where(materials.c.namespace_id == ctx.namespace_id)
            )
        ).mappings()
    }
    region_rows: dict[str, list[SourceRegion]] = {}
    for value in (
        await view.connection.execute(
            select(regions.c.record).where(regions.c.namespace_id == ctx.namespace_id)
        )
    ).scalars():
        region = SourceRegion.model_validate(value)
        region_rows.setdefault(region.material_id, []).append(region)
    result: list[MediaRecord] = []
    for row in sources:
        material = material_rows.get(row["material_id"])
        if material is None or material.get("purged"):
            continue
        source = row["record"]
        content_text: str = (material.get("text") or "") if include_text else ""
        if (
            include_text
            and material.get("text") is None
            and material_service
            and material_service.content_store
            and (material["media_type"].startswith("text/") or "json" in material["media_type"])
        ):
            async with material_service.content_store.protect(
                ContentObject(
                    material["content_hash"], material["byte_len"], material["media_type"]
                )
            ) as path:
                content_text = (await asyncio.to_thread(path.read_bytes)).decode("utf-8-sig")
        recorded = TimePoint(
            coordinate=int(datetime.fromisoformat(material["recorded_at"]).timestamp() * 1000)
        )
        times = CognitiveTimes(
            event=TimePoint.model_validate(source["event_time"])
            if source.get("event_time")
            else None,
            observation=TimePoint.model_validate(source["observed_at"]),
            formation=recorded,
            recorded=recorded,
        )
        candidates: list[SourceRegion | None] = []
        if source_permitted(ctx, row["material_id"], row["id"]) or material.get(
            "required_source_ids"
        ):
            candidates.append(None)
            candidates.extend(region_rows.get(row["material_id"], []))
        elif ctx.source_groups():
            union = {atom for group in ctx.source_groups() for atom in group}
            candidates.extend(
                region
                for region in region_rows.get(row["material_id"], [])
                if region.id in union
                or f"source-region:{region.id}" in union
                or f"source_region:{region.id}" in union
            )
        for region in candidates:
            reference = MediaReference(
                kind="source_region" if region else "material",
                id=region.id if region else row["material_id"],
                observation_id=row["id"],
            )
            if material_service is not None:
                try:
                    await material_service.media_source_in(view, ctx, reference)
                except DomainError as error:
                    if error.code not in {"source_unavailable", "region_not_found"}:
                        raise
                    continue
            text: str = content_text
            if region:
                if any(
                    value is not None
                    for value in (
                        region.text_start,
                        region.text_end,
                        region.json_pointer,
                        region.line_number,
                    )
                ):
                    text = source_region_text(text, region) if content_text else ""
                else:
                    text = ""
            text = text or f"{material['filename']} {material['media_type']}"
            provenance = (
                await material_service.provenance_in(view, ctx, reference, derivations=derivations)
                if material_service is not None
                else None
            )
            result.append(
                MediaRecord(
                    reference=reference,
                    addresses=region.addresses
                    if region
                    else ReferenceAddresses.model_validate(material["addresses"])
                    if material.get("addresses")
                    else None,
                    namespace_id=ctx.namespace_id,
                    subject_id=ctx.require_subject(),
                    material_id=row["material_id"],
                    content_hash=material["content_hash"],
                    media_type=material["media_type"],
                    byte_len=material["byte_len"],
                    filename=material["filename"],
                    text=text,
                    source_roots=provenance.roots if provenance else (row["id"],),
                    support_roots=provenance.roots if provenance else (row["id"],),
                    source_regions=provenance.regions
                    if provenance
                    else {row["id"]: (region.id,)}
                    if region
                    else {},
                    times=times,
                    region=region,
                    commit_seq=max(row["commit_seq"], material["commit_seq"]),
                )
            )
    if derivations is not None:
        result.extend(await derivations.records_in(view, ctx, include_text=include_text))
    return result


def embedding_input(record: MediaRecord) -> MediaEmbeddingInput:
    return MediaEmbeddingInput(
        reference=record.reference,
        content_hash=record.content_hash,
        byte_len=record.byte_len,
        media_type=record.media_type,
        filename=record.filename,
    )


async def source_material_ids_in(
    view: ReadView | WriteTx,
    ctx: CallContext,
    reference: MediaReference,
    materials: MaterialService,
    derivations: DerivedService | None,
) -> tuple[str, ...]:
    """Revalidate all exact media inputs and return their actual paid-call source roots."""
    if reference.kind in {"representation", "derived_region"}:
        if derivations is None:
            raise DomainError("source_unavailable", "Derived media authority is unavailable")
        return (await derivations.source_in(view, ctx, reference)).material_ids
    material, _, _ = await materials.media_source_in(view, ctx, reference)
    return (material["id"],)
