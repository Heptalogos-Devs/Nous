"""Bounded task management over the authoritative workflow directory."""

from __future__ import annotations

from collections import Counter
from typing import TYPE_CHECKING

from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from pydantic import JsonValue

from nous.platform.errors import NousError

from .scope import JobPurpose

if TYPE_CHECKING:
    from .runtime import DurableExecution


async def summarize_jobs(
    self: DurableExecution,
    ctx: CallContext,
    *,
    kind: str,
    limit: int = 20,
    purpose: JobPurpose = "actor",
    workflow_ids: tuple[str, ...] | None = None,
) -> dict[str, JsonValue]:
    if not 1 <= limit <= 1000:
        raise DomainError("invalid_limit", "Job limit must be between 1 and 1000")
    counts: Counter[str] = Counter()
    running: list[JsonValue] = []
    failed: list[JsonValue] = []
    cursor: str | None = None
    while True:
        page = await self.query_jobs(
            ctx, kind=kind, limit=1000, cursor=cursor, purpose=purpose, workflow_ids=workflow_ids
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


async def retry_jobs(
    self: DurableExecution,
    ctx: CallContext,
    *,
    kind: str,
    limit: int,
    workflow_ids: tuple[str, ...] | None = None,
    purpose: JobPurpose = "actor",
) -> int:
    page = await self.query_jobs(
        ctx, kind=kind, state="ERROR", limit=limit, workflow_ids=workflow_ids, purpose=purpose
    )
    for job in page.items:
        await self.resume(ctx, job.workflow_id, purpose=purpose)
    return len(page.items)


async def process_jobs(
    self: DurableExecution,
    ctx: CallContext,
    *,
    kind: str,
    limit: int,
    max_retry: int = 3,
    purpose: JobPurpose = "actor",
    workflow_ids: tuple[str, ...] | None = None,
) -> dict[str, JsonValue]:
    if not 1 <= limit <= 1000 or max_retry < 0:
        raise DomainError("invalid_job_batch", "Supply a bounded limit and nonnegative retry limit")
    processed = 0
    failures: list[JsonValue] = []
    items: list[JsonValue] = []
    cursor: str | None = None
    while True:
        page = await self.query_jobs(
            ctx, kind=kind, limit=1000, cursor=cursor, purpose=purpose, workflow_ids=workflow_ids
        )
        for job in page.items:
            if processed >= limit:
                break
            if job.status not in {"PENDING", "ENQUEUED", "DELAYED", "ERROR"}:
                continue
            if job.status == "ERROR" and (job.recovery_attempts or 0) >= max_retry:
                continue
            processed += 1
            try:
                if job.status == "ERROR":
                    await self.resume(ctx, job.workflow_id, purpose=purpose)
                result = await self.wait(ctx, job.workflow_id, purpose=purpose)
                items.append({"job_id": job.workflow_id, "result": result})
            except (DomainError, NousError) as error:
                failures.append(
                    {"job_id": job.workflow_id, "error": str(error), "code": error.code}
                )
        cursor = page.next_cursor
        if cursor is None or processed >= limit:
            break
    current = await self.summarize_jobs(ctx, kind=kind, purpose=purpose, workflow_ids=workflow_ids)
    counts = current["counts"]
    assert isinstance(counts, dict)
    return {
        "processed": processed,
        "unfinished": sum(
            value
            for state, value in counts.items()
            if state in {"PENDING", "ENQUEUED", "DELAYED"} and isinstance(value, int)
        ),
        "failed": len(failures),
        "failures": failures,
        "items": items,
    }
