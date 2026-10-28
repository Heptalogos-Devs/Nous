"""Typed jobs client operations."""

from __future__ import annotations

from collections.abc import Sequence

import anyio
from pydantic import TypeAdapter

from ..errors import NousError
from ..operations import object_value, segment
from ..types import (
    Job,
    JobDetail,
    JsonObject,
)
from .materials import MaterialClient


class JobClient(MaterialClient):
    async def jobs(
        self, *, namespace_id: str | None = None, subject_id: str | None = None, limit: int = 50
    ) -> tuple[Job, ...]:
        return TypeAdapter(tuple[Job, ...]).validate_python(
            await self.request(
                "GET",
                "/api/jobs",
                params={
                    "namespace_id": namespace_id or self.namespace_id,
                    "subject_id": subject_id or self.subject_id,
                    "limit": limit,
                },
            )
        )

    async def job(
        self, job_id: str, *, namespace_id: str | None = None, subject_id: str | None = None
    ) -> JobDetail:
        return JobDetail.model_validate(
            await self.request(
                "GET",
                f"/api/jobs/{segment(job_id)}",
                params={
                    "namespace_id": namespace_id or self.namespace_id,
                    "subject_id": subject_id or self.subject_id,
                },
            )
        )

    async def cancel_job(
        self, job_id: str, *, namespace_id: str | None = None, subject_id: str | None = None
    ) -> JsonObject:
        return object_value(
            await self.request(
                "POST",
                f"/api/jobs/{segment(job_id)}/cancel",
                params={
                    "namespace_id": namespace_id or self.namespace_id,
                    "subject_id": subject_id or self.subject_id,
                },
            )
        )

    async def wait_jobs(
        self,
        ids: Sequence[str],
        *,
        namespace_id: str | None = None,
        subject_id: str | None = None,
        timeout_ms: int = 45000,
    ) -> tuple[Job, ...]:
        terminal = {"SUCCESS", "ERROR", "CANCELLED", "MAX_RECOVERY_ATTEMPTS_EXCEEDED"}
        results: dict[str, Job] = {}
        interval = 0.25
        with anyio.fail_after(timeout_ms / 1000):
            while len(results) < len(set(ids)):
                for job_id in ids:
                    if job_id in results:
                        continue
                    job = (
                        await self.job(job_id, namespace_id=namespace_id, subject_id=subject_id)
                    ).job
                    if job.status in terminal:
                        results[job_id] = job
                if len(results) < len(set(ids)):
                    await anyio.sleep(interval)
                    interval = min(interval * 1.5, 2.0)
        return tuple(results[identity] for identity in ids)

    async def wait_success(
        self,
        ids: Sequence[str],
        *,
        namespace_id: str | None = None,
        subject_id: str | None = None,
        timeout_ms: int = 45000,
    ) -> tuple[Job, ...]:
        jobs = await self.wait_jobs(
            ids, namespace_id=namespace_id, subject_id=subject_id, timeout_ms=timeout_ms
        )
        for job in jobs:
            if job.status != "SUCCESS":
                raise NousError(
                    {
                        "code": "job_failed",
                        "message": f"Job {job.workflow_id} ended as {job.status}",
                        "context": {"job_id": job.workflow_id, "error": job.error},
                    }
                )
        return jobs
