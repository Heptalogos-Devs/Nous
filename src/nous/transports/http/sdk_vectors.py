"""Exact-target durable vector admission, without an implicit library scan."""

from fastapi import APIRouter, Request
from nous_wave.contracts.vector_projection import ProjectVectors

from .security import application, context

router = APIRouter()


@router.post("/namespaces/{namespace_id}/subjects/{subject_id}/embeddings/jobs")
async def project_vectors(
    request: Request, namespace_id: str, subject_id: str, command: ProjectVectors
):
    return await application(request).vector_projection.submit(
        await context(request, namespace_id, subject_id),
        command,
    )
