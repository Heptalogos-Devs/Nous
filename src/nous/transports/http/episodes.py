"""Episode operations use the canonical scoped Episode and execution services."""

from fastapi import APIRouter, Request
from nous_wave.contracts.episodes import (
    EpisodeGet,
    EpisodeQuery,
    ProcessEpisodeSources,
    RebuildEpisodes,
)

from .security import application, context

router = APIRouter(prefix="/api/namespaces/{namespace_id}/subjects/{subject_id}/episodes")


@router.post("/query")
async def query(request: Request, namespace_id: str, subject_id: str, command: EpisodeQuery):
    return await application(request).episodes.query(
        await context(request, namespace_id, subject_id, capability="read"), command
    )


@router.post("/get")
async def get(request: Request, namespace_id: str, subject_id: str, command: EpisodeGet):
    return await application(request).episodes.get(
        await context(request, namespace_id, subject_id, capability="read"),
        command.reference,
        command.paragraph_limit,
    )


@router.post("/rebuild")
async def rebuild(request: Request, namespace_id: str, subject_id: str, command: RebuildEpisodes):
    return await application(request).episodes.submit_rebuild(
        await context(request, namespace_id, subject_id), command
    )


@router.get("/status")
async def status(request: Request, namespace_id: str, subject_id: str, limit: int = 20):
    return await application(request).episodes.status(
        await context(request, namespace_id, subject_id, capability="read"), limit
    )


@router.post("/process")
async def process(
    request: Request, namespace_id: str, subject_id: str, command: ProcessEpisodeSources
):
    return await application(request).episodes.process_sources(
        await context(request, namespace_id, subject_id), command
    )


@router.get("/{episode_id}/history")
async def history(
    request: Request, namespace_id: str, subject_id: str, episode_id: str, limit: int = 100
):
    return await application(request).episodes.history(
        await context(request, namespace_id, subject_id, capability="read"), episode_id, limit
    )
