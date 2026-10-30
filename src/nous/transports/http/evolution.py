"""Atomic cognitive evolution and exact media regions."""

from uuid import uuid4

from fastapi import APIRouter, Request
from fastapi.responses import Response
from nous_sdk.contracts import (
    CognitiveRef,
    MergeConcepts,
    ResegmentEpisodes,
    SourceRegion,
    SplitConcept,
)
from nous_sdk.contracts.ego_model import AdoptSeed

from .security import application, context

router = APIRouter(prefix="/api/namespaces/{namespace_id}/subjects/{subject_id}")


@router.post("/self/seed", include_in_schema=False)
@router.post("/ego/seed")
async def seed_ego(request: Request, namespace_id: str, subject_id: str, command: AdoptSeed):
    return await application(request).cognition.adopt_seed(
        await context(request, namespace_id, subject_id), command
    )


@router.get("/self/profile", include_in_schema=False)
@router.get("/ego/profile")
async def ego_profile(request: Request, namespace_id: str, subject_id: str):
    return await application(request).cognition.ego_profile(
        await context(request, namespace_id, subject_id, capability="read")
    )


@router.post("/cognition/merge-concepts")
async def merge(request: Request, namespace_id: str, subject_id: str, command: MergeConcepts):
    return await application(request).cognition.merge_concepts(
        await context(request, namespace_id, subject_id), command
    )


@router.post("/cognition/split-concept")
async def split(request: Request, namespace_id: str, subject_id: str, command: SplitConcept):
    return await application(request).cognition.split_concept(
        await context(request, namespace_id, subject_id), command
    )


@router.post("/cognition/resegment-episodes")
async def resegment(
    request: Request, namespace_id: str, subject_id: str, command: ResegmentEpisodes
):
    return await application(request).cognition.resegment_episodes(
        await context(request, namespace_id, subject_id), command
    )


@router.post("/motivation/assess")
async def assess(request: Request, namespace_id: str, subject_id: str, reference: CognitiveRef):
    return await application(request).cognition.assess_motivation(
        await context(request, namespace_id, subject_id), reference
    )


@router.post("/regions")
async def register_region(
    request: Request, namespace_id: str, subject_id: str, region: SourceRegion
):
    app = application(request)
    ctx = await context(request, namespace_id, subject_id)
    async with app.authority.write(
        ctx, str(uuid4()), operation_kind="material.region", scope=subject_id, payload=region
    ) as tx:
        return tx.result(await app.material.register_region_in(tx, ctx, region))


@router.get("/regions/{region_id}")
async def region(request: Request, namespace_id: str, subject_id: str, region_id: str):
    return await application(request).material.read_region(
        await context(request, namespace_id, subject_id, capability="read"), region_id
    )


@router.get("/regions/{region_id}/content")
async def region_content(request: Request, namespace_id: str, subject_id: str, region_id: str):
    value = await application(request).media_imports.region_input(
        await context(request, namespace_id, subject_id, capability="read"), region_id
    )
    return Response(value.data, media_type=value.media_type)
