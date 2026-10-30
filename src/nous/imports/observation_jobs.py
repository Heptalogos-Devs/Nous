"""Durable formation requests point to accepted source events and return real jobs."""

from __future__ import annotations

from typing import TYPE_CHECKING
from uuid import uuid4

from nous_sdk.contracts import CallContext, DomainError, MediaReference
from nous_sdk.contracts.imports import FormObservation, ObservationFormSubmission
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select, update

from nous.platform.execution import JobSpec
from nous.platform.persistence import WriteTx

from .schema import requests

if TYPE_CHECKING:
    from .service import ImportService


async def submit_form_observation_in(
    self: ImportService,
    tx: WriteTx,
    ctx: CallContext,
    command: FormObservation,
    *,
    owned_parent: str | None = None,
) -> ObservationFormSubmission:
    from nous.material.service import require_observation_in

    reference = command.source
    region_id = reference.id if reference and reference.kind == "source_region" else None
    event = await require_observation_in(tx, ctx, command.observation_id, region_id=region_id)
    source = reference or MediaReference(kind="material", id=event["material_id"])
    if source.observation_id is not None and source.observation_id != command.observation_id:
        raise DomainError(
            "observation_source_mismatch", "Formation must use its actual source observation"
        )
    material, _, _ = await self.materials.media_source_in(
        tx, ctx, source.model_copy(update={"observation_id": command.observation_id})
    )
    if material["id"] != event["material_id"]:
        raise DomainError(
            "observation_source_mismatch", "Formation source belongs to a different original"
        )
    request_id = str(uuid4())
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
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="imports.form.submit",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            return ObservationFormSubmission.model_validate(tx.replay_result)
        return tx.result(await self.submit_form_observation_in(tx, ctx, command))


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
        "producer_call_id": result.producer_call_id,
        "commit_seq": result.commit_seq,
        "vector_policy": result.vector_policy,
        "vector_error": result.vector_error.model_dump(mode="json")
        if result.vector_error
        else None,
    }
