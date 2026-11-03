"""Evidence-backed Social and configuration-seed operations."""

from fastapi import APIRouter, Request
from nous_sdk.contracts.social import (
    ChangeSocialRelation,
    IdentityHypothesis,
    LinkIdentity,
    SymbolSelection,
    UnlinkIdentity,
)

from .security import application, context

router = APIRouter()


@router.post('/namespaces/{namespace_id}/social/identities/hypotheses')
async def identity_hypothesis(
    request: Request, namespace_id: str,  command: IdentityHypothesis
):
    return await application(request).social.propose_identity(
        await context(request, namespace_id), command
    )


@router.post('/namespaces/{namespace_id}/social/identities/unlink')
async def unlink_identity(
    request: Request, namespace_id: str,  command: UnlinkIdentity
):
    return await application(request).social.unlink_identity(
        await context(request, namespace_id), command
    )


@router.post('/namespaces/{namespace_id}/social/symbols/understand')
async def understand_symbol(
    request: Request, namespace_id: str,  command: SymbolSelection
):
    return await application(request).symbols.understand(
        await context(
            request,
            namespace_id,
            
            capability="read" if command.semantic == "off" else "write",
        ),
        command,
    )


@router.post('/namespaces/{namespace_id}/social/symbols/express')
async def expression_candidates(
    request: Request, namespace_id: str,  command: SymbolSelection
):
    return await application(request).symbols.expressions(
        await context(
            request,
            namespace_id,
            
            capability="read" if command.semantic == "off" else "write",
        ),
        command,
    )


@router.post('/namespaces/{namespace_id}/social/identities')
async def link_identity(
    request: Request, namespace_id: str,  command: LinkIdentity
):
    return await application(request).social.link_identity(
        await context(request, namespace_id), command
    )


@router.get('/namespaces/{namespace_id}/social/identities/resolve')
async def resolve_identity(
    request: Request, namespace_id: str,  platform: str, account_id: str
):
    return await application(request).social.resolve_identity(
        await context(request, namespace_id,  capability="read"), platform, account_id
    )


@router.post('/namespaces/{namespace_id}/social/relations')
async def change_social_relation(
    request: Request, namespace_id: str,  command: ChangeSocialRelation
):
    return await application(request).social.change_social_relation(
        await context(request, namespace_id), command
    )


@router.get('/namespaces/{namespace_id}/social/profile/{entity_id}')
async def social_profile(request: Request, namespace_id: str,  entity_id: str):
    return await application(request).social.social_profile(
        await context(request, namespace_id,  capability="read"), entity_id
    )
