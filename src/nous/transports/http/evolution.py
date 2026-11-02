"""Atomic cognitive evolution and exact media regions."""

from fastapi import APIRouter, Request
from fastapi.responses import Response
from nous_sdk.contracts import (
    CognitiveRef,
    MergeConcepts,
    ResegmentEpisodes,
    SourceRegion,
    SplitConcept,
)
from nous_sdk.contracts.ego_model import InterpretSeed
from nous_sdk.contracts.motivation import (
    AssessMotivation,
    MotivationAssessmentsPage,
    MotivationAssessmentsQuery,
    SavedMotivationAssessment,
)

from .security import application, context

router = APIRouter(prefix="/api/knowledge/namespaces/{namespace_id}")


@router.post("/ego/seed/interpret")
async def interpret_seed(request: Request, namespace_id: str, command: InterpretSeed):
    return await application(request).seeds.interpret(await context(request, namespace_id), command)


@router.post("/ego/seed/import")
async def import_seed(request: Request, namespace_id: str, command: InterpretSeed):
    return await application(request).seeds.import_and_adopt(
        await context(request, namespace_id), command
    )


@router.get("/ego/profile/markdown")
async def ego_markdown(request: Request, namespace_id: str):
    text = await application(request).seeds.markdown(
        await context(request, namespace_id, capability="read")
    )
    return Response(text, media_type="text/markdown; charset=utf-8")


@router.get("/ego/profile/view")
async def ego_view(request: Request, namespace_id: str):
    return {
        "markdown": await application(request).seeds.markdown(
            await context(request, namespace_id, capability="read")
        )
    }


@router.get("/ego/profile")
async def ego_profile(request: Request, namespace_id: str):
    return await application(request).ego.ego_profile(
        await context(request, namespace_id, capability="read")
    )


@router.post("/cognition/merge-concepts")
async def merge(request: Request, namespace_id: str, command: MergeConcepts):
    return await application(request).evolution.merge_concepts(
        await context(request, namespace_id), command
    )


@router.post("/cognition/split-concept")
async def split(request: Request, namespace_id: str, command: SplitConcept):
    return await application(request).evolution.split_concept(
        await context(request, namespace_id), command
    )


@router.post("/cognition/resegment-episodes")
async def resegment(request: Request, namespace_id: str, command: ResegmentEpisodes):
    return await application(request).evolution.resegment_episodes(
        await context(request, namespace_id), command
    )


@router.post("/motivation/assess")
async def assess(request: Request, namespace_id: str, reference: AssessMotivation | CognitiveRef):
    return await application(request).motivation.assess_motivation(
        await context(
            request,
            namespace_id,
            capability="write"
            if isinstance(reference, AssessMotivation)
            and (reference.save or reference.semantic != "off")
            else "read",
        ),
        reference,
    )


@router.post("/motivation/assessments/query")
async def assessment_history(
    request: Request, namespace_id: str, query: MotivationAssessmentsQuery
) -> MotivationAssessmentsPage:
    return await application(request).motivation.motivation_assessments(
        await context(request, namespace_id, capability="read"), query
    )


@router.get("/motivation/assessments/{assessment_id}")
async def saved_assessment(
    request: Request, namespace_id: str, assessment_id: str
) -> SavedMotivationAssessment:
    return await application(request).motivation.motivation_assessment(
        await context(request, namespace_id, capability="read"), assessment_id
    )


@router.post("/regions")
async def register_region(request: Request, namespace_id: str, region: SourceRegion):
    app = application(request)
    ctx = await context(request, namespace_id)
    async with app.authority.write(ctx) as tx:
        return await app.material.register_region_in(tx, ctx, region)


@router.get("/regions/{region_id}")
async def region(request: Request, namespace_id: str, region_id: str):
    return await application(request).material.read_region(
        await context(request, namespace_id, capability="read"), region_id
    )


@router.get("/regions/{region_id}/content")
async def region_content(request: Request, namespace_id: str, region_id: str):
    value = await application(request).media_imports.region_input(
        await context(request, namespace_id, capability="read"), region_id
    )
    return Response(value.data, media_type=value.media_type)
