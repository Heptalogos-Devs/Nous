"""Authentication constructs trusted callers before resolving narrower read scopes."""

from uuid import uuid4

from fastapi import Request
from nous_sdk.contracts import CallContext, TimePoint
from nous_sdk.contracts.common import utc_now
from nous_sdk.contracts.credentials import Capability
from nous_sdk.contracts.identity import IdentityPurpose

from nous.application import Application
from nous.identity.credentials import AuthenticationError, Principal
from nous.platform.observability import bind_call_context
from nous.platform.observability import request_id as current_request_id

from .source_scope import resolve_scope


def application(request: Request) -> Application:
    return request.app.state.application


async def authenticate_request(request: Request) -> Principal:
    header = request.headers.get("Authorization", "")
    scheme, separator, token = header.partition(" ")
    if not separator or scheme.lower() != "bearer" or not token:
        raise AuthenticationError()
    return await application(request).credentials.authenticate(token)


async def authorize(request: Request, *, capability: Capability | None = None) -> Principal:
    principal = await authenticate_request(request)
    required = capability or ("read" if request.method in {"GET", "HEAD"} else "write")
    principal.require(required)
    return principal


async def context(
    request: Request,
    namespace_id: str,
    *,
    capability: Capability | None = None,
    identity_purpose: IdentityPurpose = "active",
) -> CallContext:
    principal = await authorize(request, capability=capability)
    principal.require_scope(namespace_id)
    if identity_purpose == "management":
        principal.require("manage")
    clock = request.headers.get("x-nous-cognitive-clock")
    if clock is not None and len(clock.encode()) > 4096:
        raise ValueError("Cognitive clock header exceeds its size limit")
    source_scope = await resolve_scope(request, namespace_id)
    ctx = CallContext(
        caller_id=principal.caller_id,
        namespace_id=namespace_id,
        request_id=current_request_id() or request.headers.get("x-nous-trace-id", str(uuid4())),
        locale=request.headers.get("Accept-Language", "zh-CN"),
        clock=TimePoint.model_validate_json(clock) if clock is not None else utc_now(),
        credential_source_ids=principal.source_ids,
        allowed_source_ids=source_scope.allowed_source_ids,
        source_constraints=source_scope.source_constraints,
    )
    bind_call_context(ctx)
    admission = request.scope.get("nous.namespace_admission")
    if admission is not None and namespace_id != "instance":
        stack, admitted = admission
        if namespace_id not in admitted:
            await stack.enter_async_context(
                application(request).namespace_limits.request(
                    ctx, authority=application(request).authority
                )
            )
            admitted.add(namespace_id)
    if namespace_id != "instance" and identity_purpose == "active":
        from nous.identity.access import require_namespace_in

        async with application(request).authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
    return ctx
