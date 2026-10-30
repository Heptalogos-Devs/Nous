"""A-Memorix profiles through the formal domain owners."""

from __future__ import annotations

from collections import Counter
from typing import TYPE_CHECKING

from nous_wave.amemorix.contracts import (
    ProfileInput,
)
from nous_wave.contracts import (
    CallContext,
)
from pydantic import JsonValue

from .helpers import json_object

if TYPE_CHECKING:
    from .service import AMemorixService


async def profile(
    self: AMemorixService, ctx: CallContext, command: ProfileInput
) -> dict[str, JsonValue]:
    from .profile_admin import profile_admin

    return await profile_admin(self, ctx, {**command.model_dump(mode="json"), "action": "query"})


async def stats(self: AMemorixService, ctx: CallContext) -> dict[str, JsonValue]:
    records = await self.records(ctx)
    counts = Counter(record.reference.kind for record in records if not record.deleted)
    jobs = await self.app.execution.list_jobs(ctx)
    from .diagnostics import diagnostics

    observed = await diagnostics(self, ctx)
    retrieval = observed["retrieval"]
    assert isinstance(retrieval, dict)
    spaces = retrieval["spaces"]
    assert isinstance(spaces, list)
    vector_count = 0
    for space in spaces:
        if isinstance(space, dict):
            count = space.get("indexed_references")
            if isinstance(count, int):
                vector_count += count
    return json_object(
        {
            **observed,
            "success": True,
            "paragraphs": counts["memory"],
            "entities": counts["entity"],
            "relations": counts["association"],
            "episodes": counts["episode"],
            "cognition": dict(counts),
            "tasks": dict(Counter(job.status for job in jobs)),
            "tasks_recent": True,
            "vectors": vector_count,
        }
    )
