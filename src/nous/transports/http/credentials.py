"""Manage scoped API keys through the authenticated instance manager."""

from uuid import uuid4

from fastapi import APIRouter, Query, Request
from nous_sdk.contracts.credentials import (
    CreateCredential,
    CredentialEvent,
    CredentialRecord,
    IssuedCredential,
    PrincipalRecord,
    RotateCredential,
)

from .security import application, authenticate_request, authorize

router = APIRouter(prefix="/api/credentials")


@router.get("/whoami")
async def whoami(request: Request) -> PrincipalRecord:
    return application(request).credentials.whoami(await authenticate_request(request))


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


@router.get("/{credential_id}")
async def read(request: Request, credential_id: str) -> CredentialRecord:
    return await application(request).credentials.read(
        await authorize(request, capability="manage"), credential_id
    )


@router.get("/{credential_id}/history")
async def history(request: Request, credential_id: str) -> tuple[CredentialEvent, ...]:
    return await application(request).credentials.history(
        await authorize(request, capability="manage"), credential_id
    )


@router.post("/{credential_id}/rotate")
async def rotate(
    request: Request, credential_id: str, command: RotateCredential
) -> IssuedCredential:
    return await application(request).credentials.rotate(
        await authorize(request, capability="manage"), credential_id, command
    )


@router.delete("/{credential_id}")
async def revoke(
    request: Request,
    credential_id: str,
    operation_id: str | None = None,
    expected_revision: int | None = Query(default=None, ge=1),
    reason: str = Query(default="", max_length=4096),
) -> CredentialRecord:
    principal = await authorize(request, capability="manage")
    return await application(request).credentials.revoke(
        principal,
        credential_id,
        operation_id or str(uuid4()),
        expected_revision=expected_revision,
        reason=reason,
    )
