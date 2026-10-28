"""Public SDK resource routes backed by the shared domain owners."""

from uuid import uuid4

from fastapi import APIRouter, Request
from nous_wave.contracts import (
    BlobDocument,
    CallContext,
    CognitiveRef,
    DomainError,
    ImportDocument,
    MediaReference,
    MergeConcepts,
    QueryRequest,
    ResegmentEpisodes,
    SplitConcept,
    TimePoint,
    UnitOutput,
)
from nous_wave.contracts.context import (
    ChangeResidents,
    ChangeWorkContext,
    CognitiveTarget,
    ContextAdmissionCommand,
    ContextPrepare,
    ContextReport,
    MediaTarget,
    OpenContextSession,
)
from nous_wave.contracts.credentials import Capability
from nous_wave.contracts.lifecycle import MemoryChange
from pydantic import BaseModel, ConfigDict, Field, JsonValue, TypeAdapter
from sqlalchemy import select

from nous.cognition.schema import revisions
from nous.identity.service import subjects
from nous.imports.schema import outputs
from nous.material.service import materials, observations, require_observation_in

from .security import application, authorize, context

router = APIRouter(prefix="/api")


class OperationIdentity(BaseModel):
    model_config = ConfigDict(extra="forbid")
    operation_id: str = Field(default_factory=lambda: str(uuid4()))


class PutText(OperationIdentity):
    namespace_id: str
    subject_id: str
    text: str
    source: str | None = None
    event_time: TimePoint | None = None


class Observe(OperationIdentity):
    subject_id: str
    material_id: str
    observed_at: TimePoint | None = None
    origin: dict[str, str] = Field(default_factory=dict)


class ReadReference(BaseModel):
    model_config = ConfigDict(extra="forbid")
    subject_id: str
    reference: CognitiveRef | MediaReference | str
    observation_id: str | None = None
    as_of_commit: int | None = None


async def subject_context(
    request: Request, subject_id: str, capability: Capability | None = None
) -> CallContext:
    await authorize(request, capability=capability)
    async with application(request).authority.read() as view:
        namespace_id = (
            await view.connection.execute(
                select(subjects.c.namespace_id).where(subjects.c.id == subject_id)
            )
        ).scalar_one_or_none()
    if namespace_id is None:
        raise DomainError("subject_not_found", "Subject does not exist")
    return await context(request, str(namespace_id), subject_id, capability=capability)


async def cognitive_context(request: Request, object_id: str) -> CallContext:
    await authorize(request, capability="read")
    async with application(request).authority.read() as view:
        row = (
            await view.connection.execute(
                select(revisions.c.namespace_id, revisions.c.subject_id)
                .where(revisions.c.id == object_id)
                .limit(1)
            )
        ).first()
    if row is None:
        raise DomainError("cognition_not_found", "Cognitive object is unavailable")
    return await context(request, str(row.namespace_id), str(row.subject_id), capability="read")


async def material_context(request: Request, material_id: str) -> CallContext:
    await authorize(request, capability="read")
    async with application(request).authority.read() as view:
        rows = (
            await view.connection.execute(
                select(observations.c.namespace_id, observations.c.subject_id).where(
                    observations.c.material_id == material_id, observations.c.active.is_(True)
                )
            )
        ).all()
    for row in rows:
        try:
            ctx = await context(
                request, str(row.namespace_id), str(row.subject_id), capability="read"
            )
            await application(request).material.read(ctx, material_id)
            return ctx
        except DomainError:
            continue
    raise DomainError("source_unavailable", "Material has no authorized live observation")


async def reference_in(
    request: Request,
    ctx: CallContext,
    reference: CognitiveRef | str,
    as_of_commit: int | None = None,
) -> CognitiveRef:
    locator = reference.exact() if isinstance(reference, CognitiveRef) else reference
    resolution = await application(request).references.resolve(ctx, locator, as_of_commit)
    if not isinstance(resolution.reference, CognitiveTarget):
        raise DomainError("cognition_required", "Reference selects media rather than cognition")
    return resolution.reference.reference


@router.post("/subjects/{subject_id}/query")
async def query(request: Request, subject_id: str, command: QueryRequest):
    return await application(request).retrieval.query(
        await subject_context(request, subject_id, "read"), command
    )


@router.get("/query/manual")
async def manual(request: Request):
    from nous_native import nousql_manual

    await authorize(request, capability="read")
    return nousql_manual()


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/cognition/read")
async def cognition_read(
    request: Request, namespace_id: str, subject_id: str, command: CognitiveRef
):
    return await application(request).cognition.get(
        await context(request, namespace_id, subject_id, capability="read"), command
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/concepts/merge")
async def merge(request: Request, namespace_id: str, subject_id: str, command: MergeConcepts):
    return await application(request).cognition.merge_concepts(
        await context(request, namespace_id, subject_id), command
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/concepts/split")
async def split(request: Request, namespace_id: str, subject_id: str, command: SplitConcept):
    return await application(request).cognition.split_concept(
        await context(request, namespace_id, subject_id), command
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/episodes/resegment")
async def resegment(
    request: Request, namespace_id: str, subject_id: str, command: ResegmentEpisodes
):
    return await application(request).cognition.resegment_episodes(
        await context(request, namespace_id, subject_id), command
    )


@router.get("/memories/{object_id}")
async def memory(request: Request, object_id: str):
    ctx = await cognitive_context(request, object_id)
    reference = await reference_in(request, ctx, f"memory:{object_id}")
    return await application(request).cognition.get(ctx, reference)


@router.get("/memories/{object_id}/source")
async def memory_source(request: Request, object_id: str, revision: int | None = None):
    ctx = await cognitive_context(request, object_id)
    reference = await reference_in(
        request, ctx, f"memory:{object_id}" + (f"@{revision}" if revision is not None else "")
    )
    record = await application(request).cognition.get(ctx, reference)
    return {
        "reference": reference,
        "cognition": record,
        "source_roots": record.source_roots,
        "evidence": record.evidence,
    }


@router.post("/references")
async def resolve_reference(request: Request, command: ReadReference):
    ctx = await subject_context(request, command.subject_id, "read")
    locator = command.reference if isinstance(command.reference, str) else command.reference.exact()
    return await application(request).references.resolve(ctx, locator, command.as_of_commit)


@router.post("/sources/read")
async def source_read(request: Request, command: ReadReference):
    app = application(request)
    ctx = await subject_context(request, command.subject_id, "read")
    if isinstance(command.reference, str):
        target = (await app.references.resolve(ctx, command.reference)).reference
    elif isinstance(command.reference, CognitiveRef):
        target = CognitiveTarget(reference=command.reference)
    else:
        target = MediaTarget(reference=command.reference)
    if isinstance(target, CognitiveTarget):
        record = await app.cognition.get(ctx, target.reference)
        return {
            "reference": target.reference,
            "cognition": record,
            "source_roots": record.source_roots,
            "evidence": record.evidence,
        }
    ref = target.reference
    if ref.kind == "material":
        if command.observation_id is not None:
            async with app.authority.read(ctx) as view:
                row = await require_observation_in(view, ctx, command.observation_id)
                if row["material_id"] != ref.id:
                    raise DomainError(
                        "source_mismatch", "Observation does not belong to this original"
                    )
        return {"reference": ref, "material": await app.material.read(ctx, ref.id)}
    if ref.kind == "source_region":
        region, material = await app.material.region_source(ctx, ref.id)
        if command.observation_id is not None:
            async with app.authority.read(ctx) as view:
                row = await require_observation_in(
                    view, ctx, command.observation_id, region_id=ref.id
                )
                if row["material_id"] != region.material_id:
                    raise DomainError(
                        "source_mismatch", "Observation does not belong to this region"
                    )
        text = material["text"]
        excerpt = (
            text[region.text_start : region.text_end]
            if text is not None and region.text_start is not None and region.text_end is not None
            else None
        )
        return {"reference": ref, "region": region, "text": excerpt}
    raise DomainError("source_unavailable", "Media derivation is unavailable")


@router.get("/materials/{material_id}")
async def material_read(request: Request, material_id: str):
    return await application(request).material.read(
        await material_context(request, material_id), material_id
    )


@router.get("/materials/{material_id}/scope")
async def material_scope(request: Request, material_id: str):
    ctx = await material_context(request, material_id)
    return {"namespace_id": ctx.namespace_id, "subject_id": ctx.subject_id}


@router.post("/materials")
async def put_text(request: Request, command: PutText):
    app = application(request)
    ctx = await context(request, command.namespace_id, command.subject_id)
    document = ImportDocument(
        text=command.text, filename=command.source or "input.txt", event_time=command.event_time
    )
    async with app.authority.write(
        ctx,
        command.operation_id,
        operation_kind="material.put",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            return tx.replay_result
        return tx.result(await app.material.put_in(tx, ctx, document))


@router.post("/observations")
async def observe(request: Request, command: Observe):
    app = application(request)
    ctx = await subject_context(request, command.subject_id, "write")
    async with app.authority.write(
        ctx,
        command.operation_id,
        operation_kind="material.observe",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            return tx.replay_result
        material = (
            await tx.connection.execute(
                select(materials.c.record).where(
                    materials.c.id == command.material_id,
                    materials.c.namespace_id == ctx.namespace_id,
                )
            )
        ).scalar_one_or_none()
        if material is None:
            raise DomainError("material_not_found", "Material is unavailable in this namespace")
        document = BlobDocument(
            filename=material["filename"],
            media_type=material["media_type"],
            source_type=material["source_type"],
            origin=command.origin,
            observed_at=command.observed_at,
        )
        value = await app.material.observe_in(tx, ctx, command.material_id, document)
        return tx.result(value)


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/imports/outputs/{output_id}")
async def import_output(request: Request, namespace_id: str, subject_id: str, output_id: str):
    app = application(request)
    ctx = await context(request, namespace_id, subject_id)
    async with app.authority.read(ctx) as view:
        raw = (
            await view.connection.execute(select(outputs.c.record).where(outputs.c.id == output_id))
        ).scalar_one_or_none()
        if raw is None:
            raise DomainError("output_not_found", "Import output is unavailable")
        output = UnitOutput.model_validate(raw)
        await require_observation_in(view, ctx, output.observation_id)
        for ref in output.cognition:
            await app.cognition.get_in(view, ctx, ref)
        return output


@router.get("/jobs")
async def jobs(request: Request, namespace_id: str, subject_id: str | None = None, limit: int = 50):
    return await application(request).execution.list_jobs(
        await context(request, namespace_id, subject_id), limit=limit
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/preparations")
async def prepare_context(
    request: Request, namespace_id: str, subject_id: str, command: ContextPrepare
):
    return await application(request).contexts.prepare(
        await context(request, namespace_id, subject_id), command
    )


@router.get(
    "/namespaces/{namespace_id}/subjects/{subject_id}/contexts/preparations/{preparation_id}"
)
async def preparation(request: Request, namespace_id: str, subject_id: str, preparation_id: str):
    return await application(request).contexts.preparation(
        await context(request, namespace_id, subject_id, capability="read"), preparation_id
    )


@router.post(
    "/namespaces/{namespace_id}/subjects/{subject_id}/contexts/preparations/{preparation_id}/cancel"
)
async def cancel_preparation(
    request: Request,
    namespace_id: str,
    subject_id: str,
    preparation_id: str,
    command: OperationIdentity,
):
    return await application(request).contexts.cancel_preparation(
        await context(request, namespace_id, subject_id), preparation_id, command.operation_id
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/admissions")
async def admit_managed(
    request: Request, namespace_id: str, subject_id: str, command: ContextAdmissionCommand
):
    return await application(request).contexts.admit_managed(
        await context(request, namespace_id, subject_id), command
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/reports")
async def report_managed(
    request: Request, namespace_id: str, subject_id: str, command: ContextReport
):
    return await application(request).contexts.report_managed(
        await context(request, namespace_id, subject_id), command
    )


@router.get("/subjects/{subject_id}")
async def subject_read(request: Request, subject_id: str):
    ctx = await subject_context(request, subject_id, "read")
    async with application(request).authority.read(ctx) as view:
        row = (
            (
                await view.connection.execute(
                    select(subjects).where(
                        subjects.c.id == subject_id, subjects.c.namespace_id == ctx.namespace_id
                    )
                )
            )
            .mappings()
            .one()
        )
        return dict(row)


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/sessions")
async def open_session(
    request: Request, namespace_id: str, subject_id: str, command: OpenContextSession
):
    return await application(request).contexts.open_session(
        await context(request, namespace_id, subject_id), command.operation_id, command.session_id
    )


@router.post(
    "/namespaces/{namespace_id}/subjects/{subject_id}/contexts/sessions/{session_id}/close"
)
async def close_session(
    request: Request,
    namespace_id: str,
    subject_id: str,
    session_id: str,
    command: OperationIdentity,
):
    return await application(request).contexts.close_session(
        await context(request, namespace_id, subject_id), session_id, command.operation_id
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/memories/change")
async def memory_change(
    request: Request, namespace_id: str, subject_id: str, command: MemoryChange
):
    return await application(request).cognition.change_memory(
        await context(request, namespace_id, subject_id), command
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/preparation-jobs")
async def submit_context_preparation(
    request: Request, namespace_id: str, subject_id: str, command: ContextPrepare
):
    return await application(request).contexts.submit_prepare(
        await context(request, namespace_id, subject_id), command
    )


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/sessions/{session_id}")
async def read_context_session(
    request: Request, namespace_id: str, subject_id: str, session_id: str
):
    return await application(request).contexts.read_session(
        await context(request, namespace_id, subject_id, capability="read"), session_id
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/residents")
async def change_context_residents(
    request: Request, namespace_id: str, subject_id: str, command: ChangeResidents
):
    return await application(request).contexts.change_residents(
        await context(request, namespace_id, subject_id), command
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/work")
async def change_work_context(
    request: Request, namespace_id: str, subject_id: str, command: ChangeWorkContext
):
    return await application(request).contexts.change_work(
        await context(request, namespace_id, subject_id), command
    )


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/contexts/work/{work_context_id}")
async def read_work_context(
    request: Request, namespace_id: str, subject_id: str, work_context_id: str
):
    return await application(request).contexts.read_work(
        await context(request, namespace_id, subject_id, capability="read"), work_context_id
    )


@router.get("/subjects/{subject_id}/diagnostics")
async def subject_diagnostics(request: Request, subject_id: str):
    from nous.integrations.amemorix.service import AMemorixService

    ctx = await subject_context(request, subject_id, "read")
    app = application(request)
    summary = await AMemorixService(app).stats(ctx)
    model_calls = await app.models.ledger.list(ctx)
    usage: dict[str, int] = {}
    for call in model_calls:
        measured = TypeAdapter(JsonValue).validate_python(call.get("usage"))
        if isinstance(measured, dict):
            for key in ("input_tokens", "output_tokens", "total_tokens"):
                value = measured.get(key)
                if isinstance(value, int):
                    usage[key] = usage.get(key, 0) + value
    return {
        **summary,
        "namespace_id": ctx.namespace_id,
        "subject_id": subject_id,
        "model_calls": len(model_calls),
        "model_usage": usage,
    }
