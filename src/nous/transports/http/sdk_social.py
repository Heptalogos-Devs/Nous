"""Evidence-backed Social and configuration-seed operations."""

from fastapi import APIRouter, Request
from nous_sdk.contracts.social import ChangeSocialRelation, LinkIdentity

from .security import application, context

router = APIRouter()


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/social/identities")
async def link_identity(
    request: Request, namespace_id: str, subject_id: str, command: LinkIdentity
):
    return await application(request).cognition.link_identity(
        await context(request, namespace_id, subject_id), command
    )


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/social/identities/resolve")
async def resolve_identity(
    request: Request, namespace_id: str, subject_id: str, platform: str, account_id: str
):
    return await application(request).cognition.resolve_identity(
        await context(request, namespace_id, subject_id, capability="read"), platform, account_id
    )


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/social/relations")
async def change_social_relation(
    request: Request, namespace_id: str, subject_id: str, command: ChangeSocialRelation
):
    return await application(request).cognition.change_social_relation(
        await context(request, namespace_id, subject_id), command
    )


@router.get("/namespaces/{namespace_id}/subjects/{subject_id}/social/profile/{entity_id}")
async def social_profile(request: Request, namespace_id: str, subject_id: str, entity_id: str):
    return await application(request).cognition.social_profile(
        await context(request, namespace_id, subject_id, capability="read"), entity_id
    )
