"""Manage scoped API keys through the authenticated instance manager."""

from uuid import uuid4

from fastapi import APIRouter, Request
from nous_wave.contracts.credentials import CreateCredential, CredentialRecord, IssuedCredential

from .security import application, authorize

router = APIRouter(prefix="/api/credentials")


@router.post("")
async def create(request: Request, command: CreateCredential) -> IssuedCredential:
    principal = await authorize(request, capability="manage")
    return await application(request).credentials.create(principal, command)


@router.get("")
async def list_credentials(
    request: Request, include_revoked: bool = True
) -> tuple[CredentialRecord, ...]:
    principal = await authorize(request, capability="manage")
    return await application(request).credentials.list(principal, include_revoked=include_revoked)


@router.delete("/{credential_id}")
async def revoke(
    request: Request, credential_id: str, operation_id: str | None = None
) -> CredentialRecord:
    principal = await authorize(request, capability="manage")
    return await application(request).credentials.revoke(
        principal, credential_id, operation_id or str(uuid4())
    )
