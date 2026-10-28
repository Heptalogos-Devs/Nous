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
from nous_wave.contracts.context import ContextAdmissionCommand, ContextPrepare, ContextReport
from nous_wave.contracts.credentials import Capability
from pydantic import BaseModel, ConfigDict, Field
from sqlalchemy import select

from nous.cognition.service import revisions
from nous.identity.service import subjects
from nous.imports.service import outputs
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
    if isinstance(reference, CognitiveRef):
        return reference
    name = reference
    kind = None
    if ":" in name:
        kind, name = name.split(":", 1)
    revision = None
    if "@" in name:
        name, version = name.rsplit("@", 1)
        try:
            revision = int(version)
        except ValueError as error:
            raise DomainError(
                "reference_invalid", "Reference revision must be a positive integer"
            ) from error
    query = select(revisions.c.record).where(
        revisions.c.id == name,
        revisions.c.namespace_id == ctx.namespace_id,
        revisions.c.subject_id == ctx.subject_id,
    )
    if kind is not None:
        query = query.where(revisions.c.kind == kind)
    if revision is not None:
        query = query.where(revisions.c.revision == revision)
    if as_of_commit is not None:
        query = query.where(revisions.c.commit_seq <= as_of_commit)
    query = query.order_by(revisions.c.revision.desc()).limit(1)
    async with application(request).authority.read(ctx) as view:
        value = (await view.connection.execute(query)).scalar_one_or_none()
    if value is None:
        raise DomainError("reference_not_found", "Reference is unavailable")
    parsed = CognitiveRef.model_validate(value["reference"])
    await application(request).cognition.get(ctx, parsed)
    return parsed


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
    if isinstance(command.reference, MediaReference):
        return await source_read(request, command)
    reference = await reference_in(request, ctx, command.reference, command.as_of_commit)
    record = await application(request).cognition.get(ctx, reference)
    return {"reference": reference, "object": record}


@router.post("/sources/read")
async def source_read(request: Request, command: ReadReference):
    app = application(request)
    ctx = await subject_context(request, command.subject_id, "read")
    if isinstance(command.reference, MediaReference):
        ref = command.reference
        if ref.kind == "material":
            value = await app.material.read(ctx, ref.id)
            return {"reference": ref, "material": value}
        if ref.kind == "source_region":
            region, material = await app.material.region_source(ctx, ref.id)
            text = material["text"]
            excerpt = (
                text[region.text_start : region.text_end]
                if text is not None
                and region.text_start is not None
                and region.text_end is not None
                else None
            )
            return {"reference": ref, "region": region, "text": excerpt}
        raise DomainError("source_unavailable", "Media derivation is unavailable")
    reference = await reference_in(request, ctx, command.reference)
    record = await app.cognition.get(ctx, reference)
    return {
        "reference": reference,
        "cognition": record,
        "source_roots": record.source_roots,
        "evidence": record.evidence,
    }


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
