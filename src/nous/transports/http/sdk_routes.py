"""Public SDK resource routes backed by the shared domain owners."""

from fastapi import APIRouter, Request
from nous_wave.contracts import (
    BlobDocument,
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
    CognitiveTarget,
    MediaTarget,
)
from nous_wave.contracts.lifecycle import MemoryChange
from pydantic import BaseModel, ConfigDict, Field, JsonValue, TypeAdapter
from sqlalchemy import select

from nous.identity.service import subjects
from nous.imports.schema import outputs
from nous.material.service import materials, require_observation_in

from .sdk_cognition import router as cognition_router
from .sdk_common import (
    OperationIdentity,
    cognitive_context,
    material_context,
    reference_in,
    subject_context,
)
from .sdk_contexts import router as context_router
from .sdk_imports import router as import_router
from .sdk_media import router as media_router
from .sdk_social import router as social_router
from .sdk_sources import router as sources_router
from .security import application, authorize, context

router = APIRouter(prefix="/api")
router.include_router(context_router)
router.include_router(import_router)
router.include_router(media_router)
router.include_router(social_router)
router.include_router(sources_router)
router.include_router(cognition_router)


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
    if (
        command.observation_id is not None
        and ref.observation_id is not None
        and command.observation_id != ref.observation_id
    ):
        raise DomainError("source_mismatch", "Source request names conflicting observations")
    observation_id = command.observation_id or ref.observation_id
    ref = ref.model_copy(update={"observation_id": observation_id})
    if ref.kind == "material":
        if observation_id is not None:
            async with app.authority.read(ctx) as view:
                row = await require_observation_in(view, ctx, observation_id)
                if row["material_id"] != ref.id:
                    raise DomainError(
                        "source_mismatch", "Observation does not belong to this original"
                    )
        return {"reference": ref, "material": await app.material.read(ctx, ref.id)}
    if ref.kind == "source_region":
        region, material = await app.material.region_source(ctx, ref.id)
        if observation_id is not None:
            async with app.authority.read(ctx) as view:
                row = await require_observation_in(view, ctx, observation_id, region_id=ref.id)
                if row["material_id"] != region.material_id:
                    raise DomainError(
                        "source_mismatch", "Observation does not belong to this region"
                    )
        text = material["text"]
        from nous.material.text import source_region_text

        excerpt = (
            source_region_text(text, region)
            if text is not None
            and any(
                value is not None
                for value in (
                    region.text_start,
                    region.text_end,
                    region.json_pointer,
                    region.line_number,
                )
            )
            else None
        )
        return {"reference": ref, "region": region, "text": excerpt}
    async with app.authority.read(ctx) as view:
        info = await app.derivations.source_in(view, ctx, ref)
        selected = await app.derivations.selected_in(view, ctx, ref)
    return {
        "reference": ref,
        "source_roots": info.roots,
        "material_ids": info.material_ids,
        "access": info.access,
        "payload": selected.payload,
        "selector": selected.selector,
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
    ctx = await context(request, namespace_id, subject_id, capability="read")
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


@router.get("/job-pages")
async def job_page(
    request: Request,
    namespace_id: str,
    subject_id: str | None = None,
    limit: int = 50,
    cursor: str | None = None,
    scope: str | None = None,
    kind: str | None = None,
    state: str | None = None,
):
    page = await application(request).execution.query_jobs(
        await context(request, namespace_id, subject_id, capability="read"),
        limit=limit,
        cursor=cursor,
        scope=scope,
        kind=kind,
        state=state,
    )
    return {"items": page.items, "next_cursor": page.next_cursor}


@router.get("/jobs")
async def jobs(
    request: Request,
    namespace_id: str,
    subject_id: str | None = None,
    limit: int = 50,
    scope: str | None = None,
    kind: str | None = None,
    state: str | None = None,
):
    page = await application(request).execution.query_jobs(
        await context(request, namespace_id, subject_id, capability="read"),
        limit=limit,
        scope=scope,
        kind=kind,
        state=state,
    )
    return page.items


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


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/memories/change")
async def memory_change(
    request: Request, namespace_id: str, subject_id: str, command: MemoryChange
):
    return await application(request).cognition.change_memory(
        await context(request, namespace_id, subject_id), command
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


@router.post("/namespaces/{namespace_id}/references/reconcile")
async def reconcile_references(
    request: Request, namespace_id: str, command: OperationIdentity, subject_id: str | None = None
):
    return await application(request).references.reconcile(
        await context(request, namespace_id, subject_id), command.operation_id
    )
