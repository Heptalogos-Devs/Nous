"""Authorized original and region projections for the shared retrieval planner."""
from __future__ import annotations

from datetime import datetime

from nous_wave.contracts import (
    CallContext,
    CognitiveTimes,
    MediaEmbeddingInput,
    MediaRecord,
    MediaReference,
    SourceRegion,
    TimePoint,
)
from sqlalchemy import select

from nous.material.service import materials, observations, regions, source_permitted
from nous.platform.persistence import ReadView, WriteTx


async def media_records_in(view: ReadView | WriteTx, ctx: CallContext) -> list[MediaRecord]:
    sources = (await view.connection.execute(select(observations).where(
        observations.c.namespace_id == ctx.namespace_id,
        observations.c.subject_id == ctx.subject_id,
        observations.c.active.is_(True)))).mappings().all()
    material_rows = {row["id"]: row["record"] for row in (await view.connection.execute(
        select(materials).where(materials.c.namespace_id == ctx.namespace_id))).mappings()}
    region_rows: dict[str, list[SourceRegion]] = {}
    for value in (await view.connection.execute(select(regions.c.record).where(regions.c.namespace_id == ctx.namespace_id))).scalars():
        region = SourceRegion.model_validate(value)
        region_rows.setdefault(region.material_id, []).append(region)
    result: list[MediaRecord] = []
    for row in sources:
        material = material_rows.get(row["material_id"])
        if material is None or material.get("purged"):
            continue
        source = row["record"]
        recorded = TimePoint(coordinate=int(datetime.fromisoformat(material["recorded_at"]).timestamp() * 1000))
        times = CognitiveTimes(event=TimePoint.model_validate(source["event_time"]) if source.get("event_time") else None,
            observation=TimePoint.model_validate(source["observed_at"]), formation=recorded, recorded=recorded)
        candidates: list[SourceRegion | None] = []
        if source_permitted(ctx, row["material_id"], row["id"]):
            candidates.append(None)
            candidates.extend(region_rows.get(row["material_id"], []))
        elif ctx.allowed_source_ids is not None:
            candidates.extend(region for region in region_rows.get(row["material_id"], []) if region.id in ctx.allowed_source_ids)
        for region in candidates:
            reference = MediaReference(kind="source_region" if region else "material", id=region.id if region else row["material_id"], observation_id=row["id"])
            text: str = material.get("text") or ""
            if region:
                if region.text_start is not None:
                    text = text[region.text_start:region.text_end]
                else:
                    text = ""
            text = text or f"{material['filename']} {material['media_type']}"
            result.append(MediaRecord(reference=reference, namespace_id=ctx.namespace_id, subject_id=ctx.require_subject(),
                material_id=row["material_id"], content_hash=material["content_hash"], media_type=material["media_type"],
                byte_len=material["byte_len"], filename=material["filename"], text=text,
                source_roots=(row["id"],), support_roots=(row["id"],),
                source_regions={row["id"]: (region.id,)} if region else {},
                times=times, region=region, commit_seq=max(row["commit_seq"], material["commit_seq"])))
    return result


def embedding_input(record: MediaRecord) -> MediaEmbeddingInput:
    return MediaEmbeddingInput(reference=record.reference, content_hash=record.content_hash,
        byte_len=record.byte_len, media_type=record.media_type, filename=record.filename)
