"""Actual per-unit execution facts, separate from immutable adoption outputs."""

from __future__ import annotations

import asyncio
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager
from datetime import UTC
from typing import TYPE_CHECKING, Literal
from uuid import uuid4

from nous_wave.contracts import CallContext, DomainError, ImportUnit
from nous_wave.contracts.imports import ImportFailure, UnitAttempt
from sqlalchemy import select, update

from .schema import requests

if TYPE_CHECKING:
    from .service import ImportService


def failure(error: BaseException) -> ImportFailure:
    return ImportFailure(
        code=error.code if isinstance(error, DomainError) else type(error).__name__,
        message=str(error)[:1024]
        if isinstance(error, DomainError)
        else "Import stage failed; consult the actual job error",
    )


class AttemptRecorder:
    def __init__(self, service: ImportService, ctx: CallContext, value: UnitAttempt) -> None:
        self.service, self.ctx, self.value = service, ctx, value

    async def save(self) -> None:
        async with self.service.authority.write(
            self.ctx,
            str(uuid4()),
            operation_kind="imports.unit_attempt",
            scope=self.ctx.require_subject(),
            payload=self.value,
        ) as tx:
            row = await tx.connection.scalar(
                select(requests.c.record).where(requests.c.id == self.value.id)
            )
            if row is not None and not row.get("purged"):
                row["attempt"] = self.value.model_dump(mode="json")
                await tx.connection.execute(
                    update(requests).where(requests.c.id == self.value.id).values(record=row)
                )
            tx.result(None)

    async def advance(
        self, stage: Literal["formation", "adoption", "embedding", "complete"]
    ) -> None:
        self.value = self.value.model_copy(update={"stage": stage})
        await self.save()


@asynccontextmanager
async def record_attempt(
    service: ImportService,
    ctx: CallContext,
    unit: ImportUnit,
    operation_id: str,
    job_id: str | None,
) -> AsyncGenerator[AttemptRecorder]:
    attempt_id = str(uuid4())
    async with service.authority.write(
        ctx,
        attempt_id + ":start",
        operation_kind="imports.unit_attempt",
        scope=ctx.require_subject(),
        payload={"unit_id": unit.id, "operation_id": operation_id, "job_id": job_id},
    ) as tx:
        value = UnitAttempt(
            id=attempt_id,
            catalog_id=unit.catalog_id,
            unit_id=unit.id,
            operation_id=operation_id,
            job_id=job_id,
            started_at=tx.recorded_at,
        )
        await tx.connection.execute(
            requests.insert().values(
                id=attempt_id,
                record={
                    "kind": "unit_attempt",
                    "context": ctx.model_dump(mode="json"),
                    "material_ids": [unit.material_id],
                    "attempt": value.model_dump(mode="json"),
                },
            )
        )
        tx.result(None)
    recorder = AttemptRecorder(service, ctx, value)
    try:
        yield recorder
    except BaseException as error:
        recorder.value = recorder.value.model_copy(
            update={
                "status": "cancelled" if isinstance(error, asyncio.CancelledError) else "failed",
                "error": failure(error),
            }
        )
        raise
    else:
        recorder.value = recorder.value.model_copy(
            update={"status": "succeeded", "stage": "complete"}
        )
    finally:
        from datetime import datetime

        recorder.value = recorder.value.model_copy(
            update={"finished_at": datetime.now(UTC).isoformat()}
        )
        await recorder.save()


async def read_attempts(
    self: ImportService, ctx: CallContext, catalog_id: str
) -> tuple[UnitAttempt, ...]:
    async with self.authority.read(ctx) as view:
        await self.read_catalog_in(view, ctx, catalog_id)
        rows = (await view.connection.execute(select(requests.c.record))).scalars()
        result: list[UnitAttempt] = []
        for row in rows:
            if row.get("kind") != "unit_attempt" or row.get("purged"):
                continue
            scope = row["context"]
            if scope["namespace_id"] != ctx.namespace_id or scope["subject_id"] != ctx.subject_id:
                continue
            value = UnitAttempt.model_validate(row["attempt"])
            if value.catalog_id == catalog_id:
                result.append(value)
        return tuple(sorted(result, key=lambda value: (value.started_at, value.id)))
