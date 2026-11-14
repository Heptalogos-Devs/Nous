"""Read completed content and results through the application operation catalogue."""

import sqlite3
from collections.abc import Awaitable, Callable

from sqlalchemy.exc import IntegrityError

from nous.application.contracts import BasicEvents, CallContext, define_operation
from nous.common.errors import NousError
from nous.storage.database import WriteTx

from .artifacts import ArtifactStore
from .contracts import (
    AcceptContent,
    Artifact,
    ContentReadView,
    ContentSelection,
    DeleteResult,
    ExportContent,
    ExportOffer,
    ReadArtifact,
    ReadResult,
    ReadSelections,
    ResultRef,
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
DELETE_RESULT = define_operation(
    "content.delete_result",
    request=DeleteResult,
    result=ResultRef,
    events=BasicEvents,
    description="Delete a completed result that is not referenced by another saved item.",
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

    def __init__(
        self,
        results: ResultStore,
        artifacts: ArtifactStore,
        release_result: Callable[[WriteTx, str], Awaitable[None]],
    ) -> None:
        self.results, self.artifacts = results, artifacts
        self.release_result = release_result

    async def read_result(self, request: ReadResult, call: CallContext) -> ResultView:
        return ResultView.model_validate(
            await self.results.read_view(
                request.ref, sources=request.sources, details=request.details
            )
        )

    async def delete_result(self, request: DeleteResult, call: CallContext) -> ResultRef:
        try:
            async with self.results.database.write() as tx:
                ref = await self.results.delete_in(tx, request.ref)
                await self.release_result(tx, ref.id)
            return ref
        except IntegrityError as error:
            if (
                getattr(error.orig, "sqlite_errorcode", None)
                == sqlite3.SQLITE_CONSTRAINT_FOREIGNKEY
            ):
                raise NousError(
                    "content.result_in_use", "The result is referenced by another saved item"
                ) from error
            raise

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
