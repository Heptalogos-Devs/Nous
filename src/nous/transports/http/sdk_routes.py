"""Public SDK resource routes backed by the shared domain owners."""

from typing import Literal

from fastapi import APIRouter, Request
from nous_sdk.contracts import (
    BlobDocument,
    CognitiveRef,
    DomainError,
    ImportDocument,
    MergeConcepts,
    QueryRequest,
    ResegmentEpisodes,
    SplitConcept,
    TimePoint,
    UnitOutput,
)
from nous_sdk.contracts.context import (
    CognitiveTarget,
    MediaTarget,
)
from nous_sdk.contracts.diagnostics import NamespaceDiagnostics
from nous_sdk.contracts.lifecycle import MemoryChange
from nous_sdk.contracts.references import (
    EvidenceTarget,
    ExactReference,
    ObservationTarget,
    ResourceTarget,
)
from nous_sdk.contracts.source_graph import ExpandSources
from nous_sdk.contracts.source_objects import EvidenceRelationRecord
from pydantic import BaseModel, ConfigDict, Field, JsonValue, TypeAdapter
from sqlalchemy import select

from nous.imports.schema import outputs
from nous.material.service import require_observation_in

from .sdk_cognition import router as cognition_router
from .sdk_common import (
    OperationIdentity,
    cognitive_context,
    material_context,
    reference_in,
    subject_context,
)
from .sdk_contexts import router as context_router
from .sdk_graph import router as graph_router
from .sdk_imports import router as import_router
from .sdk_media import router as media_router
from .sdk_social import router as social_router
from .sdk_sources import router as sources_router
from .sdk_vectors import router as vectors_router
from .security import application, authorize, context

router = APIRouter(prefix="/api/v1")
router.include_router(context_router)
router.include_router(import_router)
router.include_router(media_router)
router.include_router(social_router)
router.include_router(sources_router)
router.include_router(graph_router)
router.include_router(cognition_router)
router.include_router(vectors_router)


class PutText(OperationIdentity):
    namespace_id: str
    
    text: str
    source: str | None = None
    event_time: TimePoint | None = None


class Observe(OperationIdentity):
    namespace_id: str
    
    material_id: str
    observed_at: TimePoint | None = None
    origin: dict[str, str] = Field(default_factory=dict)


class ReadReference(BaseModel):
    namespace_id: str
    model_config = ConfigDict(extra="forbid")
    
    reference: ExactReference | str
    observation_id: str | None = None
    as_of_commit: int | None = None




@router.get("/query/manual")
async def manual(request: Request):
    from nous_native import nousql_manual

    await authorize(request, capability="read")
    return nousql_manual()


@router.post('/namespaces/{namespace_id}/cognition/read')
async def cognition_read(
    request: Request, namespace_id: str,  command: CognitiveRef
):
    return await application(request).cognition.get(
        await context(request, namespace_id,  capability="read"), command
    )


@router.post('/namespaces/{namespace_id}/concepts/merge')
async def merge(request: Request, namespace_id: str,  command: MergeConcepts):
    return await application(request).evolution.merge_concepts(
        await context(request, namespace_id), command
    )


@router.post('/namespaces/{namespace_id}/concepts/split')
async def split(request: Request, namespace_id: str,  command: SplitConcept):
    return await application(request).evolution.split_concept(
        await context(request, namespace_id), command
    )


@router.post('/namespaces/{namespace_id}/episodes/resegment')
async def resegment(
    request: Request, namespace_id: str,  command: ResegmentEpisodes
):
    return await application(request).evolution.resegment_episodes(
        await context(request, namespace_id), command
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
    ctx = await subject_context(request, command.namespace_id, "read")
    locator = command.reference if isinstance(command.reference, str) else command.reference.exact()
    return await application(request).references.resolve(ctx, locator, command.as_of_commit)


@router.post("/sources/read")
async def source_read(request: Request, command: ReadReference):
    app = application(request)
    ctx = await subject_context(request, command.namespace_id, "read")
    locator = command.reference if isinstance(command.reference, str) else command.reference.exact()
    target = (await app.references.resolve(ctx, locator, command.as_of_commit)).reference
    if isinstance(target, CognitiveTarget):
        record = await app.cognition.get(ctx, target.reference)
        return {
            "reference": target.reference,
            "cognition": record,
            "source_roots": record.source_roots,
            "evidence": record.evidence,
            "graph": await app.retrieval.expand_sources(ctx, ExpandSources(targets=(target,))),
        }
    if isinstance(target, (ResourceTarget, ObservationTarget, EvidenceTarget)):
        async with app.authority.read(ctx) as view:
            if isinstance(target, ResourceTarget):
                record = await app.resources.get_in(
                    view, ctx, target.reference.id, target.reference.revision
                )
                sources = (
                    (MediaTarget(reference=record.snapshot.source),) if record.snapshot else ()
                )
                result = {
                    "reference": record.reference,
                    "resource": record,
                    "source_roots": record.source_roots,
                }
            else:
                record = await app.references.read_source_object_in(view, ctx, target.reference)
                sources = (MediaTarget(reference=record.source),) if record.source else ()
                if isinstance(record, EvidenceRelationRecord):
                    sources = (CognitiveTarget(reference=record.target), *sources)
                result = {
                    "reference": record.reference,
                    "observation" if isinstance(target, ObservationTarget) else "evidence": record,
                    "source_roots": record.source_roots,
                }
        return {
            **result,
            "graph": await app.retrieval.expand_sources(ctx, ExpandSources(targets=sources))
            if sources
            else None,
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
    return {"namespace_id": ctx.namespace_id}


@router.post("/materials")
async def put_text(request: Request, command: PutText):
    app = application(request)
    ctx = await context(request, command.namespace_id)
    document = ImportDocument(
        text=command.text,
        filename=command.source or "input.txt",
        source=command.source,
        event_time=command.event_time,
    )
    lease = await app.content.publish_bytes(command.text.encode(), document.media_type)
    try:
        async with app.authority.write(
            ctx,
            command.operation_id,
            operation_kind="material.put",
            scope=ctx.namespace_id,
            payload=command,
        ) as tx:
            if tx.replayed:
                cached = TypeAdapter(dict[str, JsonValue]).validate_python(tx.replay_result)
                identifier = TypeAdapter(str).validate_python(cached["id"])
                effective = (
                    (
                        await app.material.restore_input_in(
                            tx,
                            ctx,
                            (identifier,),
                            operation_id=command.operation_id,
                            operation_kind="material.put",
                            scope=ctx.namespace_id,
                        )
                    ).context
                    if ctx.source_groups()
                    else ctx
                )
                return await app.material.read_in(tx, effective, identifier)
            admission = await app.material.admit_text_in(tx, ctx, document, lease)
            return tx.result(admission.material)
    finally:
        await app.content.release(lease)


@router.post("/observations")
async def observe(request: Request, command: Observe):
    app = application(request)
    ctx = await subject_context(request, command.namespace_id, "write")
    async with app.authority.write(
        ctx,
        command.operation_id,
        operation_kind="material.observe",
        scope=ctx.namespace_id,
        payload=command,
    ) as tx:
        if tx.replayed:
            cached = TypeAdapter(dict[str, JsonValue]).validate_python(tx.replay_result)
            await app.material.read_in(tx, ctx, command.material_id)
            effective = (
                (
                    await app.material.restore_input_in(
                        tx,
                        ctx,
                        (command.material_id,),
                        operation_id=command.operation_id,
                        operation_kind="material.observe",
                        scope=ctx.namespace_id,
                    )
                ).context
                if ctx.source_groups()
                else ctx
            )
            identifier = TypeAdapter(str).validate_python(cached["id"])
            event = await require_observation_in(tx, effective, identifier)
            return event["record"]
        material = await app.material.read_in(tx, ctx, command.material_id)
        document = BlobDocument(
            filename=material["filename"],
            media_type=material["media_type"],
            source_type=material["source_type"],
            source=material.get("source"),
            origin=command.origin,
            observed_at=command.observed_at,
        )
        admission = await app.material.admit_observation_in(tx, ctx, command.material_id, document)
        return tx.result(admission.event)


@router.get('/namespaces/{namespace_id}/imports/outputs/{output_id}')
async def import_output(request: Request, namespace_id: str,  output_id: str):
    app = application(request)
    ctx = await context(request, namespace_id,  capability="read")
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
    
    limit: int = 50,
    cursor: str | None = None,
    scope: str | None = None,
    kind: str | None = None,
    state: str | None = None,
    purpose: Literal["actor", "management"] = "actor",
):
    page = await application(request).execution.query_jobs(
        await context(
            request,
            namespace_id,
            
            capability="manage" if purpose == "management" else "read",
        ),
        limit=limit,
        cursor=cursor,
        scope=scope,
        kind=kind,
        state=state,
        purpose=purpose,
    )
    return {"items": page.items, "next_cursor": page.next_cursor}


@router.get("/jobs")
async def jobs(
    request: Request,
    namespace_id: str,
    
    limit: int = 50,
    scope: str | None = None,
    kind: str | None = None,
    state: str | None = None,
    purpose: Literal["actor", "management"] = "actor",
):
    page = await application(request).execution.query_jobs(
        await context(
            request,
            namespace_id,
            
            capability="manage" if purpose == "management" else "read",
        ),
        limit=limit,
        scope=scope,
        kind=kind,
        state=state,
        purpose=purpose,
    )
    return page.items




@router.post('/namespaces/{namespace_id}/memories/change')
async def memory_change(
    request: Request, namespace_id: str,  command: MemoryChange
):
    return await application(request).memories.change_memory(
        await context(request, namespace_id), command
    )


@router.get('/namespaces/{namespace_id}/diagnostics')
async def subject_diagnostics(request: Request, namespace_id: str) -> NamespaceDiagnostics:
    ctx = await subject_context(request, namespace_id, "read")
    return await application(request).diagnostics.read(ctx)


@router.post("/namespaces/{namespace_id}/references/reconcile")
async def reconcile_references(
    request: Request, namespace_id: str, command: OperationIdentity
):
    return await application(request).references.reconcile(
        await context(request, namespace_id), command.operation_id
    )
