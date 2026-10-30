"""Exact cognitive lookup and lifecycle share their formal domain owner."""

from fastapi import APIRouter, Request
from nous_wave.contracts import CognitiveRef, DomainError
from nous_wave.contracts.cognition import CognitiveKind
from nous_wave.contracts.lifecycle import MemoryChanges
from nous_wave.contracts.person_profiles import PersonProfileChange, RefreshPersonProfile
from pydantic import TypeAdapter
from sqlalchemy import select

from nous.cognition.schema import revisions

from .sdk_common import OperationIdentity, cognitive_context
from .security import application, context

router = APIRouter()


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/person-profiles/change")
async def change_person_profile(
    request: Request,
    namespace_id: str,
    subject_id: str,
    command: PersonProfileChange,
):
    return await application(request).person_profiles.change(
        await context(request, namespace_id, subject_id, capability="write"),
        command,
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/person-profiles/refresh")
async def refresh_person_profile(
    request: Request,
    namespace_id: str,
    subject_id: str,
    command: RefreshPersonProfile,
):
    return await application(request).person_profiles.submit_refresh(
        await context(request, namespace_id, subject_id, capability="write"),
        command,
    )


class CognitiveDeletion(OperationIdentity):
    reference: CognitiveRef
    deleted: bool


@router.get("/cognition/{object_id}")
async def cognitive_object(request: Request, object_id: str, revision: int | None = None):
    app = application(request)
    ctx = await cognitive_context(request, object_id)
    if revision is None:
        history = await app.cognition.history(ctx, object_id, 1)
        if not history:
            raise DomainError("cognition_not_found", "Cognitive object is unavailable")
        return history[0]
    async with app.authority.read(ctx) as view:
        kind = await view.connection.scalar(
            select(revisions.c.kind).where(revisions.c.id == object_id).limit(1)
        )
        reference = CognitiveRef(
            kind=TypeAdapter(CognitiveKind).validate_python(kind), id=object_id, revision=revision
        )
        return await app.cognition.get_in(view, ctx, reference)


@router.get("/cognition/{object_id}/history")
async def cognitive_history(request: Request, object_id: str, limit: int = 100):
    return await application(request).cognition.history(
        await cognitive_context(request, object_id), object_id, limit
    )


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/concepts/resolve")
async def concept_alias(request: Request, namespace_id: str, subject_id: str, alias: str):
    return await application(request).cognition.concept_by_alias(
        await context(request, namespace_id, subject_id, capability="read"), alias
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/cognition/deleted")
async def set_cognitive_deleted(
    request: Request, namespace_id: str, subject_id: str, command: CognitiveDeletion
):
    return await application(request).cognition.set_deleted(
        await context(request, namespace_id, subject_id),
        command.reference,
        command.deleted,
        command.operation_id,
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/memories/batch")
async def change_memories(
    request: Request, namespace_id: str, subject_id: str, command: MemoryChanges
):
    return await application(request).cognition.change_memories(
        await context(request, namespace_id, subject_id), command
    )
