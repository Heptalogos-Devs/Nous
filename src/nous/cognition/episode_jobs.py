"""Episode job controls reuse the single execution directory, wait and resume owners."""

from __future__ import annotations

from collections import Counter
from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.episodes import ProcessEpisodeSources
from pydantic import JsonValue

from nous.platform.errors import NousError

if TYPE_CHECKING:
    from .episodes import EpisodeService


async def status(self: EpisodeService, ctx: CallContext, limit: int = 20) -> dict[str, JsonValue]:
    if not 1 <= limit <= 1000:
        raise DomainError("invalid_limit", "Episode job limit must be between 1 and 1000")
    counts: Counter[str] = Counter()
    running: list[JsonValue] = []
    failed: list[JsonValue] = []
    cursor: str | None = None
    while True:
        page = await self.execution.query_jobs(
            ctx, kind="nous.episodes.rebuild", limit=1000, cursor=cursor
        )
        for job in page.items:
            counts[job.status] += 1
            item: dict[str, JsonValue] = {"job_id": job.workflow_id, "status": job.status}
            if job.status in {"PENDING", "ENQUEUED", "DELAYED"} and len(running) < limit:
                running.append(item)
            elif job.status in {"ERROR", "MAX_RECOVERY_ATTEMPTS_EXCEEDED"} and len(failed) < limit:
                failed.append(item)
        cursor = page.next_cursor
        if cursor is None:
            break
    return {"counts": dict(counts), "running": running, "failed": failed}


async def process_sources(
    self: EpisodeService, ctx: CallContext, command: ProcessEpisodeSources
) -> dict[str, JsonValue]:
    processed = 0
    rebuilt = 0
    failures: list[JsonValue] = []
    results: list[JsonValue] = []
    cursor: str | None = None
    while True:
        page = await self.execution.query_jobs(
            ctx, kind="nous.episodes.rebuild", limit=1000, cursor=cursor
        )
        for job in page.items:
            if processed >= command.limit:
                break
            if job.status in {"SUCCESS", "CANCELLED", "MAX_RECOVERY_ATTEMPTS_EXCEEDED"}:
                continue
            if job.status == "ERROR":
                if (job.recovery_attempts or 0) >= command.max_retry:
                    continue
                await self.execution.resume(ctx, job.workflow_id)
            processed += 1
            try:
                result = await self.execution.wait(ctx, job.workflow_id)
                results.append(result)
                count = result.get("rebuilt") if isinstance(result, dict) else None
                if isinstance(count, int):
                    rebuilt += count
            except NousError as error:
                failures.append(
                    {"job_id": job.workflow_id, "error": str(error), "code": error.code}
                )
        cursor = page.next_cursor
        if cursor is None or processed >= command.limit:
            break
    current = await status(self, ctx)
    counts = current["counts"]
    assert isinstance(counts, dict)
    unfinished = sum(
        value
        for state, value in counts.items()
        if state in {"PENDING", "ENQUEUED", "DELAYED"} and isinstance(value, int)
    )
    return {
        "processed": processed,
        "unfinished": unfinished,
        "rebuilt": rebuilt,
        "failed": len(failures),
        "failures": failures,
        "results": results,
    }
