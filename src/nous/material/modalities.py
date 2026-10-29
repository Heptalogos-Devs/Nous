"""Classify actual source materials for all query consumers."""

from __future__ import annotations

from collections.abc import Mapping

from nous_wave.contracts import CallContext, MediaInfo
from nous_wave.contracts.query_conditions import QueryModality
from pydantic import JsonValue
from sqlalchemy import select

from nous.platform.persistence import ReadView, WriteTx


def material_modality(record: Mapping[str, JsonValue]) -> QueryModality:
    details = record.get("media_info")
    if details is not None:
        return MediaInfo.model_validate(details).kind
    mime = record.get("media_type")
    if not isinstance(mime, str):
        return "binary"
    mime = mime.split(";", 1)[0].strip().lower()
    kinds: tuple[QueryModality, ...] = ("image", "audio", "video", "text")
    for kind in kinds:
        if mime.startswith(kind + "/"):
            return kind
    if mime in {
        "application/pdf",
        "application/msword",
        "application/rtf",
        "application/vnd.ms-excel",
        "application/vnd.ms-powerpoint",
        "application/epub+zip",
    } or mime.startswith(
        ("application/vnd.openxmlformats-officedocument.", "application/vnd.oasis.opendocument.")
    ):
        return "document"
    return "binary"


async def source_modalities_in(
    view: ReadView | WriteTx,
    ctx: CallContext,
    authorized_observation_ids: frozenset[str],
) -> dict[str, QueryModality]:
    from .service import materials, observations

    if not authorized_observation_ids:
        return {}
    rows = (
        await view.connection.execute(
            select(observations.c.id, materials.c.record)
            .join(materials, materials.c.id == observations.c.material_id)
            .where(
                observations.c.namespace_id == ctx.namespace_id,
                observations.c.subject_id == ctx.require_subject(),
                materials.c.namespace_id == ctx.namespace_id,
                observations.c.id.in_(authorized_observation_ids),
            )
        )
    ).mappings()
    return {str(row["id"]): material_modality(row["record"]) for row in rows}
