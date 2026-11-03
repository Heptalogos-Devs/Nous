"""Consume explicit context-use events into derived retrieval accessibility gains."""

from __future__ import annotations

import math
from datetime import UTC, datetime
from typing import TYPE_CHECKING

from nous.platform.call_context import CallContext
from nous_sdk.contracts.targets import CognitiveTarget
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select

if TYPE_CHECKING:
    from .generations import Manifest
    from .service import RetrievalService


def meaningful(event: dict[str, JsonValue]) -> bool:
    return bool(
        event.get("subject_view")
        and event.get("kind") in {"actual_use", "behavior_adoption", "outcome"}
        and event.get("use_kind")
        in {None, "acted_on", "result_supported", "result_refuted", "corrected"}
    )


async def consume(
    self: RetrievalService,
    ctx: CallContext,
    manifest: Manifest,
    watermark: int,
    half_life_days: float,
) -> None:
    from nous.context.models import feedback

    day = int(datetime.now(UTC).timestamp() // 86400)
    if manifest.usage_day and day > manifest.usage_day:
        decay = math.exp2(-(day - manifest.usage_day) / half_life_days)
        manifest.usage = {key: value * decay for key, value in manifest.usage.items()}
    async with self.authority.read(ctx) as view:
        values = (
            await view.connection.execute(
                select(feedback.c.record).where(
                    feedback.c.record["namespace_id"].as_string() == ctx.namespace_id,
                    feedback.c.record["commit_seq"].as_integer() > manifest.usage_through,
                    feedback.c.record["commit_seq"].as_integer() <= watermark,
                )
            )
        ).scalars()
        for event in values:
            if not meaningful(event):
                continue
            target = (
                TypeAdapter(CognitiveTarget).validate_python(event["target"])
                if event.get("target", {}).get("kind") == "cognition"
                else None
            )
            if target is not None:
                manifest.usage[target.reference.id] = (
                    manifest.usage.get(target.reference.id, 0.0) + 1.0
                )
    manifest.usage_through = watermark
    manifest.usage_day = day
