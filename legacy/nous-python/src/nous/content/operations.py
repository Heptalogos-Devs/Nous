"""Read completed content and results through the application operation catalogue."""

from nous.application.contracts import BasicEvents, CallContext, define_operation

from .artifacts import ArtifactStore
from .contracts import (
    AcceptContent,
    Artifact,
    ContentReadView,
    ContentSelection,
    ExportContent,
    ExportOffer,
    ReadArtifact,
    ReadResult,
    ReadSelections,
    ResultView,
)
from .results import ResultStore

READ_RESULT = define_operation(
    "content.read_result",
    request=ReadResult,
    result=ResultView,
    events=BasicEvents,
    description="Read a completed result, its outputs and selected and consumed sources.",
)
READ_SELECTIONS = define_operation(
    "content.read_selections",
    request=ReadSelections,
    result=tuple[ContentReadView, ...],
    events=BasicEvents,
    description="Read selected content and its existing representations.",
)
READ_ARTIFACT = define_operation(
    "content.read_artifact",
    request=ReadArtifact,
    result=ContentReadView,
    events=BasicEvents,
    description="Read an accepted artifact and its existing representations.",
)
ACCEPT_CONTENT = define_operation(
    "content.accept",
    request=AcceptContent,
    result=Artifact,
    events=BasicEvents,
    description="Accept supplied content as an artifact.",
)
EXPORT_CONTENT = define_operation(
    "content.export_offer",
    request=ExportContent,
    result=ExportOffer,
    events=BasicEvents,
    description="Prepare an artifact for file export.",
)


class ContentOperations:
    """Present persisted results and exact selections without invoking their producers."""

    def __init__(self, results: ResultStore, artifacts: ArtifactStore) -> None:
        self.results, self.artifacts = results, artifacts

    async def read_result(self, request: ReadResult, call: CallContext) -> ResultView:
        return ResultView.model_validate(
            await self.results.read_view(
                request.ref, sources=request.sources, details=request.details
            )
        )

    async def read_selections(
        self, request: ReadSelections, call: CallContext
    ) -> tuple[ContentReadView, ...]:
        return await self.artifacts.read_view_many(
            request.selections, representations=request.representations
        )

    async def read_artifact(self, request: ReadArtifact, call: CallContext) -> ContentReadView:
        artifact = await self.artifacts.get(request.ref)
        return (
            await self.artifacts.read_view_many(
                (ContentSelection(artifact=artifact.ref, selection=request.selection),),
                representations=request.representations,
            )
        )[0]

    async def accept(self, request: AcceptContent, call: CallContext) -> Artifact:
        artifact = await call.inputs.accept(request.key, name=request.name)
        await call.saved(artifact.ref)
        return artifact

    async def export_offer(self, request: ExportContent, call: CallContext) -> ExportOffer:
        artifact = await self.artifacts.get(request.ref)
        return ExportOffer(
            artifact_ref=artifact.ref,
            suggested_name=artifact.display_name,
            media_type=artifact.media_type,
        )
