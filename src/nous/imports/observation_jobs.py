"""Durable formation requests point to accepted source events and return real jobs."""

from __future__ import annotations

from hashlib import sha256
from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.imports import FormObservation, ObservationFormSubmission
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select, update

from nous.platform.execution import JobSpec
from nous.platform.persistence import WriteTx

from .schema import requests

if TYPE_CHECKING:
    from .service import ImportService


def formation_request_id(ctx: CallContext, command: FormObservation) -> str:
    return (
        "observation:"
        + sha256(f"{ctx.namespace_id}:{ctx.caller_id}:{command.operation_id}".encode()).hexdigest()
    )


async def observation_jobs(
    self: ImportService, ctx: CallContext, observation_ids: tuple[str, ...]
) -> tuple[str, ...]:
    async with self.authority.read(ctx) as view:
        for identifier in observation_ids:
            await self.materials.accepted_source_in(view, ctx, identifier)
        values = (
            await view.connection.execute(
                select(requests.c.record).where(
                    requests.c.record["context"]["namespace_id"].as_string() == ctx.namespace_id,
                    requests.c.record["kind"].as_string() == "observation_form",
                    requests.c.record["command"]["observation_id"].as_string().in_(observation_ids),
                )
            )
        ).scalars()
        return tuple(
            str(value["job_id"])
            for value in values
            if value.get("job_id") and not value.get("purged")
        )


async def submit_form_observation_in(
    self: ImportService,
    tx: WriteTx,
    ctx: CallContext,
    command: FormObservation,
    *,
    owned_parent: str | None = None,
) -> ObservationFormSubmission:
    selected = await self.materials.accepted_source_in(tx, ctx, command.observation_id, command.source)
    material = selected.material
    request_id = formation_request_id(ctx, command)
    existing = await tx.connection.scalar(
        select(requests.c.record).where(requests.c.id == request_id)
    )
    if existing is not None:
        if existing["command"] != command.model_dump(mode="json"):
            raise DomainError(
                "formation_request_conflict", "Formation request has different accepted input"
            )
        return ObservationFormSubmission(
            request_id=request_id, job_id=existing["job_id"], observation_id=command.observation_id
        )

    await tx.connection.execute(
        requests.insert().values(
            id=request_id,
            record={
                "kind": "observation_form",
                "context": ctx.model_dump(mode="json"),
                "command": command.model_dump(mode="json"),
                "material_ids": [material["id"]],
            },
        )
    )
    job = await tx.enqueue(
        JobSpec(
            workflow_name="nous.imports.form_observation",
            queue_name="formation",
            args=(request_id,),
            owned_parent=owned_parent,
            scope=command.observation_id,
        )
    )
    row = await tx.connection.scalar(select(requests.c.record).where(requests.c.id == request_id))
    await tx.connection.execute(
        update(requests)
        .where(requests.c.id == request_id)
        .values(
            record={
                **TypeAdapter(dict[str, JsonValue]).validate_python(row),
                "job_id": job.workflow_id,
            }
        )
    )
    return ObservationFormSubmission(
        request_id=request_id, job_id=job.workflow_id, observation_id=command.observation_id
    )


async def submit_form_observation(
    self: ImportService, ctx: CallContext, command: FormObservation
) -> ObservationFormSubmission:
    async with self.authority.write(ctx) as tx:
        return await self.submit_form_observation_in(tx, ctx, command)


async def run_form_observation(self: ImportService, request_id: str):
    async with self.authority.read() as view:
        row = await view.connection.scalar(
            select(requests.c.record).where(requests.c.id == request_id)
        )
    if row is None or row.get("kind") != "observation_form":
        raise DomainError("formation_request_not_found", "Formation request is unavailable")
    if row.get("purged"):
        raise DomainError("source_purged", "Formation source has been purged")
    result = await self.form_observation(
        CallContext.model_validate(row["context"]), FormObservation.model_validate(row["command"])
    )
    return {
        "observation_id": result.observation_id,
        "cognition": [reference.model_dump(mode="json") for reference in result.cognition],
        "representations": [reference.model_dump(mode="json") for reference in result.representations],
        "commit_seq": result.commit_seq,
        "vector_policy": result.vector_policy,
        "vector_error": result.vector_error.model_dump(mode="json")
        if result.vector_error
        else None,
    }
