"""Exact cognitive lookup and lifecycle share their formal domain owner."""

from fastapi import APIRouter, Request
from nous_sdk.contracts import CognitiveRef, DomainError, MergeEntities, SplitEntity
from nous_sdk.contracts.cognition import CognitiveKind
from nous_sdk.contracts.lifecycle import MemoryChanges
from nous_sdk.contracts.person_profiles import (
    PersonProfileChange,
    PersonProfilesQuery,
    PersonProfilesResult,
    RefreshPersonProfile,
)
from nous_sdk.contracts.records import ReadRecords, RecordPurpose, RecordsQuery
from pydantic import BaseModel, ConfigDict, Field, TypeAdapter
from sqlalchemy import select

from nous.cognition.schema import revisions

from .sdk_common import OperationIdentity
from .security import application, context

router = APIRouter()


@router.post('/namespaces/{namespace_id}/person-profiles/query')
async def person_profiles(
    request: Request, namespace_id: str,  command: PersonProfilesQuery
) -> PersonProfilesResult:
    return await application(request).person_profiles.query(
        await context(request, namespace_id,  capability="read"), command
    )


@router.post('/namespaces/{namespace_id}/entities/merge')
async def merge_entities(
    request: Request, namespace_id: str,  command: MergeEntities
):
    return await application(request).entities.merge_entities(
        await context(request, namespace_id,  capability="manage"), command
    )


@router.post('/namespaces/{namespace_id}/entities/split')
async def split_entity(request: Request, namespace_id: str,  command: SplitEntity):
    return await application(request).entities.split_entity(
        await context(request, namespace_id,  capability="manage"), command
    )


class RecordDetailRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    reference: CognitiveRef
    history_limit: int = Field(default=100, ge=1, le=1000)
    purpose: RecordPurpose = "exact"


@router.post('/namespaces/{namespace_id}/records/query')
async def records_query(
    request: Request, namespace_id: str,  command: RecordsQuery
):
    return await application(request).cognition.records(
        await context(
            request,
            namespace_id,
            
            capability="manage" if command.purpose == "management" else "read",
        ),
        command,
    )


@router.post('/namespaces/{namespace_id}/records/read')
async def records_read(request: Request, namespace_id: str,  command: ReadRecords):
    return await application(request).cognition.read_records(
        await context(
            request,
            namespace_id,
            
            capability="manage" if command.purpose == "management" else "read",
        ),
        command,
    )


@router.post('/namespaces/{namespace_id}/records/detail')
async def record_detail(
    request: Request, namespace_id: str,  command: RecordDetailRequest
):
    return await application(request).cognition.record_detail(
        await context(
            request,
            namespace_id,
            
            capability="manage" if command.purpose == "management" else "read",
        ),
        command.reference,
        history_limit=command.history_limit,
        purpose=command.purpose,
    )


@router.post('/namespaces/{namespace_id}/person-profiles/change')
async def change_person_profile(
    request: Request,
    namespace_id: str,
    
    command: PersonProfileChange,
):
    return await application(request).person_profiles.change(
        await context(request, namespace_id,  capability="write"),
        command,
    )


@router.post('/namespaces/{namespace_id}/person-profiles/refresh')
async def refresh_person_profile(
    request: Request,
    namespace_id: str,
    
    command: RefreshPersonProfile,
):
    return await application(request).person_profiles.submit_refresh(
        await context(request, namespace_id,  capability="write"),
        command,
    )


class CognitiveDeletion(OperationIdentity):
    reference: CognitiveRef
    deleted: bool


@router.get("/namespaces/{namespace_id}/cognition/{object_id}")
async def cognitive_object(request: Request, namespace_id: str, object_id: str, revision: int | None = None):
    app = application(request)
    ctx = await context(request, namespace_id, capability="read")
    if revision is None:
        history = await app.cognition.history(ctx, object_id, 1)
        if not history:
            raise DomainError("cognition_not_found", "Cognitive object is unavailable")
        return history[0]
    async with app.authority.read(ctx) as view:
        kind = await view.connection.scalar(
            select(revisions.c.kind).where(revisions.c.namespace_id == ctx.namespace_id, revisions.c.id == object_id).limit(1)
        )
        reference = CognitiveRef(
            kind=TypeAdapter(CognitiveKind).validate_python(kind), id=object_id, revision=revision
        )
        return await app.cognition.get_in(view, ctx, reference)


@router.get("/namespaces/{namespace_id}/cognition/{object_id}/history")
async def cognitive_history(request: Request, namespace_id: str, object_id: str, limit: int = 100):
    return await application(request).cognition.history(
        await context(request, namespace_id, capability="read"), object_id, limit
    )


@router.get('/namespaces/{namespace_id}/concepts/resolve')
async def concept_alias(request: Request, namespace_id: str,  alias: str):
    return await application(request).cognition.concept_by_alias(
        await context(request, namespace_id,  capability="read"), alias
    )


@router.post('/namespaces/{namespace_id}/cognition/deleted')
async def set_cognitive_deleted(
    request: Request, namespace_id: str,  command: CognitiveDeletion
):
    return await application(request).sources.set_deleted(
        await context(request, namespace_id),
        command.reference,
        command.deleted,
        command.operation_id,
    )


@router.post('/namespaces/{namespace_id}/memories/batch')
async def change_memories(
    request: Request, namespace_id: str,  command: MemoryChanges
):
    return await application(request).memories.change_memories(
        await context(request, namespace_id), command
    )
