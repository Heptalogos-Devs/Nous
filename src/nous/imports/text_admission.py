"""Record imports coordinate actual literal and semantic jobs on the shared owners."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import AdmitFiles
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.imports import ObservationFormation, ObservationFormSubmission
from nous_sdk.contracts.text_import import (
    TextImport,
    TextImportCompletion,
    TextImportResult,
    TextImportSubmission,
)
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select, update

from nous.identity.access import require_namespace_in
from nous.platform.execution import JobSpec
from nous.platform.persistence import ReadView, WriteTx

from .schema import requests

if TYPE_CHECKING:
    from .service import ImportService


def admission_command(command: TextImport) -> AdmitFiles:
    inputs = tuple(
        document.model_copy(update={"origin": {**(command.origin or {}), **document.origin}})
        for document in command.inputs
    )
    has_formation = command.formation_purpose is not None
    vector_policy = command.vectors or ("metadata_allowed" if has_formation else "disabled")
    return AdmitFiles.model_validate(
        {
            "operation_id": command.operation_id + ":records",
            "inputs": [document.model_dump(mode="json") for document in inputs],
            "record_units": True,
            "llm_enabled": has_formation,
            "deferred_formation": False,
            "formation_purpose": command.formation_purpose or "memory",
            "vectors": vector_policy,
            "dedupe_policy": "none",
        }
    )


async def request_in(
    view: ReadView | WriteTx, ctx: CallContext, request_id: str
) -> dict[str, JsonValue]:
    value = await view.connection.scalar(
        select(requests.c.record).where(requests.c.id == request_id)
    )
    if value is None or value.get("kind") != "text_import":
        raise DomainError("import_request_not_found", "Text import request is unavailable")
    saved = CallContext.model_validate(value["context"])
    if (saved.caller_id, saved.namespace_id, saved.namespace_id) != (
        ctx.caller_id,
        ctx.namespace_id,
        ctx.namespace_id,
    ):
        raise DomainError("import_request_not_found", "Text import request is unavailable")
    if value.get("purged"):
        raise DomainError("source_purged", "Text import source was purged")
    return TypeAdapter(dict[str, JsonValue]).validate_python(value)


async def submit_text_import(
    self: ImportService, ctx: CallContext, command: TextImport
) -> TextImportSubmission:
    from hashlib import sha256
    from nous.platform.persistence.authority import canonical_json

    request_id = (
        "text:"
        + sha256(f"{ctx.namespace_id}:{ctx.caller_id}:{command.operation_id}".encode()).hexdigest()
    )
    submitted_digest = sha256(canonical_json(command).encode()).hexdigest()
    leases = []
    try:
        for item in command.inputs:
            leases.append(
                await self.content_store.publish_bytes(item.inline_text().encode(), item.media_type)
                if item.material_id is None
                else None
            )
        async with self.authority.write(ctx) as tx:
            await require_namespace_in(tx, ctx, "memory")
            existing = await tx.connection.scalar(
                select(requests.c.record).where(requests.c.id == request_id)
            )
            if existing is not None:
                if existing.get("submitted_digest") != submitted_digest:
                    raise DomainError(
                        "import_request_conflict",
                        "Text import identity has different actual inputs",
                    )
                if existing.get("purged"):
                    raise DomainError("source_purged", "Text import source is unavailable")
                return TextImportSubmission(
                    request_id=request_id, job_id=existing["job_id"], completion=command.completion
                )
            effective = ctx
            documents = []
            captured_events = {}
            total_bytes = 0
            for item, lease in zip(command.inputs, leases, strict=True):
                if item.material_id is None:
                    admission = await self.materials.admit_text_in(tx, effective, item, lease)
                    effective = admission.context
                    material = admission.material
                    captured_events[material["id"]] = admission.event["id"]
                    documents.append(
                        item.model_copy(update={"text": None, "material_id": material["id"]})
                    )
                else:
                    material = await self.materials.read_in(tx, ctx, item.material_id)
                    documents.append(item)
                total_bytes += material["byte_len"]
            if total_bytes > 4 * 1024 * 1024:
                raise DomainError("import_size_limit", "Combined text import inputs exceed 4 MiB")
            captured_command = command.model_copy(update={"inputs": tuple(documents)})
            job = await tx.enqueue(
                JobSpec(
                    "nous.imports.text",
                    "default",
                    args=(request_id,),
                    workflow_id="import:" + request_id,
                    scope=ctx.namespace_id,
                )
            )
            await tx.connection.execute(
                requests.insert().values(
                    id=request_id,
                    record={
                        "kind": "text_import",
                        "context": effective.model_dump(mode="json"),
                        "command": captured_command.model_dump(mode="json"),
                        "submitted_digest": submitted_digest,
                        "job_id": job.workflow_id,
                        "material_ids": [item.material_id for item in documents],
                        "input_observations": captured_events,
                    },
                )
            )
            return TextImportSubmission(
                request_id=request_id, job_id=job.workflow_id, completion=command.completion
            )
    finally:
        for lease in leases:
            if lease is not None:
                await self.content_store.release(lease)


async def read_text_import(
    self: ImportService, ctx: CallContext, request_id: str
) -> TextImportResult:
    async with self.authority.read(ctx) as view:
        value = await request_in(view, ctx, request_id)
    outcome = value.get("outcome")
    if not isinstance(outcome, dict):
        raise DomainError(
            "import_result_pending", "Text import has not reached its requested boundary"
        )
    admission_id = value.get("admission_request_id")
    if not isinstance(admission_id, str):
        raise DomainError("import_result_pending", "Literal admission has not completed")
    admission = await self.read_admit_result(ctx, admission_id)
    return TextImportResult(
        request_id=request_id,
        job_id=TypeAdapter(str).validate_python(value["job_id"]),
        completion=TypeAdapter(TextImportCompletion).validate_python(outcome["completion"]),
        admission=admission,
        formation_jobs=TypeAdapter(tuple[ObservationFormSubmission, ...]).validate_python(
            outcome.get("formation_jobs", [])
        ),
        formations=TypeAdapter(tuple[ObservationFormation, ...]).validate_python(
            outcome.get("formations", [])
        ),
    )


async def run_text_import(self: ImportService, request_id: str) -> dict[str, JsonValue]:
    async with self.authority.read() as view:
        raw = await view.connection.scalar(
            select(requests.c.record).where(requests.c.id == request_id)
        )
    if raw is None or raw.get("kind") != "text_import" or raw.get("purged"):
        raise DomainError("source_purged", "Text import source is unavailable")
    ctx = CallContext.model_validate(raw["context"])
    command = TextImport.model_validate(raw["command"])
    execution = self.authority.execution
    if execution is None:
        raise DomainError("execution_unavailable", "Text imports require durable execution")
    parent_job_id = TypeAdapter(str).validate_python(raw["job_id"])
    submitted = TypeAdapter(dict[str, JsonValue]).validate_python(
        await self.accept_admit(
            ctx,
            admission_command(command),
            enqueue=False,
            attempt_job_id=parent_job_id,
            parent_request_id=request_id,
        )
    )
    admission_id = TypeAdapter(str).validate_python(submitted["request_id"])
    async with self.authority.read(ctx) as view:
        accepted = await view.connection.scalar(
            select(requests.c.record).where(requests.c.id == admission_id)
        )
        if accepted is None:
            raise DomainError("import_request_not_found", "Literal admission is unavailable")
        ctx = await self.input_context_in(view, ctx, accepted)
    admission = await self.finish_admit(ctx, admission_id)
    formations = tuple(
        output.formation for output in admission.outputs if output.formation is not None
    )
    formed: list[ObservationFormation] = []
    if command.completion == "formed":
        for child in formations:
            actual = await execution.wait(ctx, child.job_id)
            formed.append(ObservationFormation.model_validate(actual))
        for output in admission.outputs:
            if output.formation is None:
                if output.projection is not None:
                    await execution.wait(ctx, output.projection.job_id)
                formed.append(
                    ObservationFormation(
                        observation_id=output.observation_id,
                        cognition=output.cognition,
                        representations=output.representations,
                        commit_seq=output.commit_seq,
                        projection=output.projection,
                        vector_policy=output.vector_policy,
                    )
                )
    result: dict[str, JsonValue] = {
        "completion": command.completion,
        "formation_jobs": [item.model_dump(mode="json") for item in formations],
        "formations": [item.model_dump(mode="json") for item in formed],
    }
    async with self.authority.write(ctx) as tx:
        current = await request_in(tx, ctx, request_id)
        await tx.connection.execute(
            update(requests)
            .where(requests.c.id == request_id)
            .values(record={**current, "outcome": result})
        )
    # Adopted content is read through its formal sources and cognitive references.
    return {"request_id": request_id, "completion": command.completion}
