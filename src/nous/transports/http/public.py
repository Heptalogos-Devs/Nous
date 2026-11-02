"""Public cognitive operations over one authenticated Namespace."""

from typing import Literal

from fastapi import APIRouter, Request
from nous_sdk.contracts import CognitiveRecord, QueryRequest, QueryResult
from nous.platform.errors import DomainError
from nous_sdk.contracts.context import CognitiveTarget
from nous_sdk.contracts.corrections import CorrectionResult
from nous_sdk.contracts.public import CorrectionInput, ReadRequest, ReadView, RememberInput
from nous_sdk.contracts.text_import import TextImport, TextImportSubmission
from nous_sdk.presentation import QueryView

from .security import application, authorize, context

router = APIRouter(prefix="/api/knowledge/namespaces/{namespace_id}", tags=["knowledge"])


async def bound_context(
    request: Request, namespace_id: str, capability: Literal["read", "write", "manage"] = "read"
):
    await authorize(request, capability=capability)
    namespace = await application(request).identity.resolve(namespace_id)
    return await context(request, namespace.id, capability=capability)


@router.post("/query", summary="Search cognition and source material")
async def query(
    request: Request,
    namespace_id: str,
    command: QueryRequest,
    view: Literal["read", "raw", "trace"] = "read",
) -> QueryResult | QueryView | dict:
    app = application(request)
    ctx = await bound_context(request, namespace_id)
    result = await app.retrieval.query(ctx, command)
    result = await app.references.retain_query(ctx, result)
    if view == "raw":
        return result
    if view == "trace":
        return {
            "view": result.view().model_dump(mode="json"),
            "execution": result.execution.model_dump(mode="json") if result.execution else None,
            "plan": result.plan,
        }
    return result.view()


@router.post("/read", summary="Read an exact object or source selection")
async def read(request: Request, namespace_id: str, command: ReadRequest) -> ReadView:
    return await application(request).reading.read(
        await bound_context(request, namespace_id), command
    )


@router.post("/memories", summary="Remember explicit content with its actual source")
async def remember(request: Request, namespace_id: str, command: RememberInput) -> CognitiveRecord:
    return await application(request).memories.remember(
        await bound_context(request, namespace_id, "write"), command
    )


@router.post("/corrections", summary="Correct a referenced cognitive object")
async def correct(
    request: Request, namespace_id: str, command: CorrectionInput
) -> CorrectionResult:
    app = application(request)
    ctx = await bound_context(request, namespace_id, "write")
    target = (await app.references.resolve(ctx, command.ref)).reference
    if not isinstance(target, CognitiveTarget):
        raise DomainError("cognition_required", "Correction requires a cognitive object reference")
    return await app.corrections.correct(ctx, target.reference, command)


@router.get("/objects/{ref}/history", summary="Read the revisions of a cognitive object")
async def history(
    request: Request, namespace_id: str, ref: str, limit: int = 100
) -> tuple[CognitiveRecord, ...]:
    app = application(request)
    ctx = await bound_context(request, namespace_id)
    target = (await app.references.resolve(ctx, ref)).reference
    if not isinstance(target, CognitiveTarget):
        raise DomainError("cognition_required", "History requires a cognitive object reference")
    return await app.cognition.history(ctx, target.reference.id, limit)


@router.post("/ingestions", summary="Capture text and optionally form long-term cognition")
async def ingest(request: Request, namespace_id: str, command: TextImport) -> TextImportSubmission:
    return await application(request).imports.submit_text_import(
        await bound_context(request, namespace_id, "write"), command
    )


@router.get(
    "/materials/{ref}/representations",
    summary="Read the saved representations of a source material",
)
async def representations(request: Request, namespace_id: str, ref: str):
    return await application(request).derivations.for_material(
        await bound_context(request, namespace_id), ref.removeprefix("material:")
    )
