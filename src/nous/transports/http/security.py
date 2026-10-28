"""Authentication constructs trusted callers before resolving narrower read scopes."""

from uuid import uuid4

from fastapi import HTTPException, Request
from nous_wave.contracts import CallContext
from nous_wave.contracts.credentials import Capability

from nous.application import Application
from nous.identity.credentials import AuthenticationError, Principal

from .source_scope import resolve_scope


def application(request: Request) -> Application:
    return request.app.state.application


async def authorize(request: Request, *, capability: Capability | None = None) -> Principal:
    header = request.headers.get("Authorization", "")
    scheme, separator, token = header.partition(" ")
    if not separator or scheme.lower() != "bearer" or not token:
        raise HTTPException(status_code=401, detail="Invalid instance credential")
    try:
        principal = await application(request).credentials.authenticate(token)
    except AuthenticationError as error:
        raise HTTPException(status_code=401, detail=str(error)) from None
    required = capability or ("read" if request.method in {"GET", "HEAD"} else "write")
    principal.require(required)
    return principal


async def context(
    request: Request,
    namespace_id: str,
    subject_id: str | None = None,
    *,
    capability: Capability | None = None,
) -> CallContext:
    principal = await authorize(request, capability=capability)
    principal.require_scope(namespace_id, subject_id)
    return CallContext(
        caller_id=principal.caller_id,
        namespace_id=namespace_id,
        subject_id=subject_id,
        request_id=request.headers.get("x-nous-trace-id", str(uuid4())),
        locale=request.headers.get("Accept-Language", "zh-CN"),
        allowed_source_ids=await resolve_scope(
            request, namespace_id, subject_id, base_scope=principal.source_ids
        ),
    )
