"""Public SDK resource routes backed by the shared domain owners."""

from uuid import uuid4

from fastapi import Request
from nous_wave.contracts import (
    CallContext,
    CognitiveRef,
    DomainError,
)
from nous_wave.contracts.context import (
    CognitiveTarget,
)
from nous_wave.contracts.credentials import Capability
from pydantic import BaseModel, ConfigDict, Field
from sqlalchemy import select

from nous.cognition.schema import revisions
from nous.identity.service import subjects
from nous.material.service import observations

from .security import application, authorize, context


class OperationIdentity(BaseModel):
    model_config = ConfigDict(extra="forbid")
    operation_id: str = Field(default_factory=lambda: str(uuid4()))


async def subject_context(
    request: Request, subject_id: str, capability: Capability | None = None
) -> CallContext:
    await authorize(request, capability=capability)
    async with application(request).authority.read() as view:
        namespace_id = (
            await view.connection.execute(
                select(subjects.c.namespace_id).where(subjects.c.id == subject_id)
            )
        ).scalar_one_or_none()
    if namespace_id is None:
        raise DomainError("subject_not_found", "Subject does not exist")
    return await context(request, str(namespace_id), subject_id, capability=capability)


async def cognitive_context(request: Request, object_id: str) -> CallContext:
    await authorize(request, capability="read")
    async with application(request).authority.read() as view:
        row = (
            await view.connection.execute(
                select(revisions.c.namespace_id, revisions.c.subject_id)
                .where(revisions.c.id == object_id)
                .limit(1)
            )
        ).first()
    if row is None:
        raise DomainError("cognition_not_found", "Cognitive object is unavailable")
    return await context(request, str(row.namespace_id), str(row.subject_id), capability="read")


async def material_context(request: Request, material_id: str) -> CallContext:
    await authorize(request, capability="read")
    async with application(request).authority.read() as view:
        rows = (
            await view.connection.execute(
                select(observations.c.namespace_id, observations.c.subject_id).where(
                    observations.c.material_id == material_id, observations.c.active.is_(True)
                )
            )
        ).all()
    for row in rows:
        try:
            ctx = await context(
                request, str(row.namespace_id), str(row.subject_id), capability="read"
            )
            await application(request).material.read(ctx, material_id)
            return ctx
        except DomainError:
            continue
    raise DomainError("source_unavailable", "Material has no authorized live observation")


async def reference_in(
    request: Request,
    ctx: CallContext,
    reference: CognitiveRef | str,
    as_of_commit: int | None = None,
) -> CognitiveRef:
    locator = reference.exact() if isinstance(reference, CognitiveRef) else reference
    resolution = await application(request).references.resolve(ctx, locator, as_of_commit)
    if not isinstance(resolution.reference, CognitiveTarget):
        raise DomainError("cognition_required", "Reference selects media rather than cognition")
    return resolution.reference.reference
