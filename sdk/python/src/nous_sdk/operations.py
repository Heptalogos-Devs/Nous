"""Typed operations over the service's domain contracts."""

from collections.abc import Mapping
from typing import Literal, TypeVar
from urllib.parse import quote

from pydantic import BaseModel, JsonValue, TypeAdapter

from .contracts import (
    AdmitFiles,
    AdoptCognition,
    AdoptMedia,
    CatalogView,
    CognitiveRecord,
    CognitiveRef,
    FreezeCatalog,
    ImportResult,
    MergeConcepts,
    ProcessCatalog,
    ResegmentEpisodes,
    SourceCommand,
    SourcePreview,
    SourceResult,
    SplitConcept,
    UnitOutput,
)
from .contracts.feedback import (
    EnqueueFeedback,
    FeedbackPage,
    FeedbackQuery,
    FeedbackTask,
    RetryFeedback,
    RollbackFeedback,
)
from .contracts.lifecycle import MemoryChange
from .contracts.management import SourceSelection
from .contracts.retention import (
    DecayRetention,
    RetentionChange,
    RetentionChanges,
    RetentionDecayResult,
    RetentionSnapshot,
)
from .contracts.vector_projection import ProjectionReceipt
from .contracts.vector_spaces import (
    CheckVectorSpaces,
    DeleteVectorSpace,
    PersistenceState,
    RebuildVectors,
    RecoverVectors,
    RetryVectorProjection,
    VectorProjectionProgress,
    VectorRebuild,
    VectorRecovery,
    VectorSpaceCheck,
    VectorSpaceDeleted,
    VectorSpaces,
)
from .types import ImportSubmission, JsonObject, QueryParam, Source

T = TypeVar("T")
_JSON = TypeAdapter(JsonValue)
_OBJECT = TypeAdapter(JsonObject)


def encoded(value: BaseModel) -> JsonValue:
    return _JSON.validate_json(value.model_dump_json())


def object_value(value: JsonValue) -> JsonObject:
    return _OBJECT.validate_python(value)


def segment(value: str) -> str:
    return quote(value, safe="")


class TypedOperations:
    async def decay_retention(self, namespace_id: str,  command: DecayRetention) -> RetentionDecayResult:
        return RetentionDecayResult.model_validate(await self.request("POST", self.namespace_path(namespace_id) + "/retention/decay", body=encoded(command)))

    async def change_retention(self, namespace_id: str,  command: RetentionChange) -> CognitiveRecord:
        return CognitiveRecord.model_validate(await self.request("POST", self.namespace_path(namespace_id) + "/retention/change", body=encoded(command)))

    async def change_retentions(self, namespace_id: str,  command: RetentionChanges) -> tuple[CognitiveRecord, ...]:
        return TypeAdapter(tuple[CognitiveRecord,...]).validate_python(await self.request("POST", self.namespace_path(namespace_id) + "/retention/change-many", body=encoded(command)))

    async def retention_snapshot(self, namespace_id: str,  reference: CognitiveRef) -> RetentionSnapshot:
        return RetentionSnapshot.model_validate(await self.request("POST", self.namespace_path(namespace_id) + "/retention/snapshot", body=encoded(reference)))

    async def recover_vectors(
        self, namespace_id: str,  command: RecoverVectors
    ) -> VectorRecovery:
        return VectorRecovery.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/vectors/recover",
                body=encoded(command),
            )
        )

    async def vector_spaces(
        self, namespace_id: str,  *, include_media: bool = False
    ) -> VectorSpaces:
        path = self.namespace_path(namespace_id)
        return VectorSpaces.model_validate(
            await self.request(
                "GET", path + "/vector-spaces", params={"include_media": include_media}
            )
        )

    async def delete_vector_space(
        self, namespace_id: str, command: DeleteVectorSpace
    ) -> VectorSpaceDeleted:
        return VectorSpaceDeleted.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/vector-spaces/delete",
                body=encoded(command),
            )
        )

    async def rebuild_vectors(
        self, namespace_id: str,  command: RebuildVectors
    ) -> VectorRebuild:
        return VectorRebuild.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/vectors/rebuild",
                body=encoded(command),
            )
        )

    async def check_vector_spaces(
        self, namespace_id: str,  command: CheckVectorSpaces
    ) -> VectorSpaceCheck:
        return VectorSpaceCheck.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/vectors/check",
                body=encoded(command),
            )
        )

    async def vector_projection_progress(
        self, namespace_id: str,  request_id: str
    ) -> VectorProjectionProgress:
        return VectorProjectionProgress.model_validate(
            await self.request(
                "GET",
                self.namespace_path(namespace_id)
                + f"/vectors/projections/{segment(request_id)}",
            )
        )

    async def retry_vector_projection(
        self, namespace_id: str,  command: RetryVectorProjection
    ) -> ProjectionReceipt:
        return ProjectionReceipt.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/vectors/retry",
                body=encoded(command),
            )
        )

    async def persistence_state(
        self, namespace_id: str
    ) -> PersistenceState:
        path = self.namespace_path(namespace_id) + "/vectors/persistence"
        return PersistenceState.model_validate(await self.request("GET", path))

    async def retry_feedback(
        self, namespace_id: str,  command: RetryFeedback
    ) -> FeedbackTask:
        return FeedbackTask.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/feedback/retry",
                body=encoded(command),
            )
        )

    async def enqueue_feedback(
        self, namespace_id: str,  command: EnqueueFeedback
    ) -> FeedbackTask:
        return FeedbackTask.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/feedback",
                body=encoded(command),
            )
        )

    async def feedback_task(self, namespace_id: str,  task_id: str) -> FeedbackTask:
        return FeedbackTask.model_validate(
            await self.request(
                "GET", self.namespace_path(namespace_id) + f"/feedback/{segment(task_id)}"
            )
        )

    async def feedback_tasks(
        self, namespace_id: str,  command: FeedbackQuery | None = None
    ) -> FeedbackPage:
        return FeedbackPage.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/feedback/query",
                body=encoded(command or FeedbackQuery()),
            )
        )

    async def rollback_feedback(
        self, namespace_id: str,  command: RollbackFeedback
    ) -> FeedbackTask:
        return FeedbackTask.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/feedback/rollback",
                body=encoded(command),
            )
        )


    async def ego_profile(self, namespace_id: str) -> tuple[CognitiveRecord, ...]:
        return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
            await self.request("GET", self.namespace_path(namespace_id) + "/ego/profile")
        )

    async def request(
        self,
        method: str,
        path: str,
        *,
        body: JsonValue = None,
        params: Mapping[str, QueryParam] | None = None,
        trace_id: str | None = None,
    ) -> JsonValue:
        raise NotImplementedError

    @staticmethod
    def namespace_path(namespace_id: str) -> str:
        return f"/api/knowledge/namespaces/{segment(namespace_id)}"

    async def freeze_catalog(
        self, namespace_id: str,  command: FreezeCatalog
    ) -> CatalogView:
        return CatalogView.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/imports/catalogs",
                body=encoded(command),
            )
        )

    async def import_catalog(
        self, namespace_id: str,  catalog_id: str
    ) -> CatalogView:
        return CatalogView.model_validate(
            await self.request(
                "GET",
                self.namespace_path(namespace_id)
                + f"/imports/catalogs/{segment(catalog_id)}",
            )
        )

    async def submit_catalog(
        self, namespace_id: str,  command: ProcessCatalog
    ) -> ImportSubmission:
        return ImportSubmission.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/imports/process",
                body=encoded(command),
            )
        )

    async def admit_documents(
        self, namespace_id: str,  command: AdmitFiles
    ) -> ImportResult:
        return ImportResult.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/imports/files",
                body=encoded(command),
            )
        )

    async def import_unit_output(
        self, namespace_id: str,  output_id: str
    ) -> UnitOutput:
        return UnitOutput.model_validate(
            await self.request(
                "GET",
                self.namespace_path(namespace_id)
                + f"/imports/outputs/{segment(output_id)}",
            )
        )

    async def adopt_cognition(
        self, namespace_id: str,  command: AdoptCognition
    ) -> CognitiveRecord:
        return CognitiveRecord.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/cognition",
                body=encoded(command),
            )
        )

    async def merge_concepts(
        self, namespace_id: str,  command: MergeConcepts
    ) -> tuple[CognitiveRecord, ...]:
        return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/concepts/merge",
                body=encoded(command),
            )
        )

    async def split_concept(
        self, namespace_id: str,  command: SplitConcept
    ) -> tuple[CognitiveRecord, ...]:
        return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/concepts/split",
                body=encoded(command),
            )
        )

    async def resegment_episodes(
        self, namespace_id: str,  command: ResegmentEpisodes
    ) -> tuple[CognitiveRecord, ...]:
        return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/episodes/resegment",
                body=encoded(command),
            )
        )

    async def source_directory(self, namespace_id: str) -> tuple[Source, ...]:
        return TypeAdapter(tuple[Source, ...]).validate_python(
            await self.request("GET", self.namespace_path(namespace_id) + "/sources")
        )

    async def preview_sources(
        self,
        namespace_id: str,
        
        sources: tuple[str, ...] = (),
        *,
        selection: SourceSelection | None = None,
        action: Literal["trash", "restore", "purge"] = "trash",
    ) -> SourcePreview:
        return SourcePreview.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/sources/preview",
                body=encoded(selection or SourceSelection(observation_ids=sources)),
                params={"action": action},
            )
        )

    async def trash_sources(
        self, namespace_id: str,  command: SourceCommand
    ) -> SourceResult:
        return SourceResult.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/sources/trash",
                body=encoded(command),
            )
        )

    async def submit_source_trash(
        self, namespace_id: str,  command: SourceCommand
    ) -> str:
        result = object_value(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/sources/trash/jobs",
                body=encoded(command),
            )
        )
        return TypeAdapter(str).validate_python(result["job_id"])

    async def restore_sources(
        self, namespace_id: str,  command: SourceCommand
    ) -> SourceResult:
        return SourceResult.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/sources/restore",
                body=encoded(command),
            )
        )

    async def purge_sources(
        self, namespace_id: str,  command: SourceCommand
    ) -> SourceResult:
        return SourceResult.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/sources/purge",
                body=encoded(command),
            )
        )

    async def update_memory(
        self, namespace_id: str,  command: MemoryChange
    ) -> CognitiveRecord:
        return CognitiveRecord.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/memories/change",
                body=encoded(command),
            )
        )

    async def adopt_media(
        self, namespace_id: str,  command: AdoptMedia
    ) -> JsonObject:
        return object_value(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/media/adopt",
                body=encoded(command),
            )
        )
