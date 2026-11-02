"""Typed operations over the service's domain contracts."""

from collections.abc import Mapping
from typing import Literal, TypeVar
from urllib.parse import quote

from pydantic import BaseModel, JsonValue, TypeAdapter

from .contracts import (
    AdmitFiles,
    AdoptCognition,
    AdoptMedia,
    BundleRequest,
    CatalogView,
    CognitiveRecord,
    CognitiveRef,
    FreezeCatalog,
    ImportResult,
    InstallBundle,
    MergeConcepts,
    ProcessCatalog,
    QueryRequest,
    QueryResult,
    ResegmentEpisodes,
    SourceCommand,
    SourcePreview,
    SourceResult,
    SplitConcept,
    UnitOutput,
)
from .contracts.context import (
    ChangeContextSession,
    ChangeResidents,
    ChangeWorkContext,
    ContextAdmission,
    ContextAdmissionCommand,
    ContextCommand,
    ContextFeedback,
    ContextModelCall,
    ContextModelPlan,
    ContextModelPreview,
    ContextModelResult,
    ContextPreparation,
    ContextPreparationJob,
    ContextPrepare,
    ContextReport,
    ContextReportResult,
    ContextSession,
    OpenContextSession,
    WorkContext,
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
from .contracts.model_calls import RetainedModelOutput
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
from .types import ContextRecord, ImportSubmission, JsonObject, QueryParam, Source

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

    async def retained_model_output(self, namespace_id: str,  call_id: str, *, management: bool = False) -> RetainedModelOutput:
        return RetainedModelOutput.model_validate(await self.request("GET", f"/api/v1/models/calls/{segment(call_id)}/output", params={"namespace_id": namespace_id,  "management": management}))

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
        path = (
            self.namespace_path(namespace_id)
            if namespace_id
            else f"/api/v1/namespaces/{segment(namespace_id)}"
        )
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
                f"/api/v1/namespaces/{segment(namespace_id)}/vector-spaces/delete",
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
        path = (
            self.namespace_path(namespace_id) + "/vectors/persistence"
            if namespace_id
            else f"/api/v1/namespaces/{segment(namespace_id)}/persistence"
        )
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

    async def preview_context_model(
        self, namespace_id: str,  command: ContextModelPreview
    ) -> ContextModelPlan:
        return ContextModelPlan.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/contexts/model-preview",
                body=encoded(command),
            )
        )

    async def invoke_context_model(
        self, namespace_id: str,  command: ContextModelCall
    ) -> ContextModelResult:
        return ContextModelResult.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/contexts/model-invocations",
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
        return f"/api/v1/namespaces/{segment(namespace_id)}"

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

    async def query_cognition(
        self, namespace_id: str,  command: QueryRequest
    ) -> QueryResult:
        return QueryResult.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/query",
                body=encoded(command),
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

    async def cognitive_revision(
        self, namespace_id: str,  reference: CognitiveRef
    ) -> CognitiveRecord:
        return CognitiveRecord.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/cognition/read",
                body=encoded(reference),
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

    async def export_bundle(
        self, namespace_id: str,  command: BundleRequest | None = None
    ) -> JsonObject:
        return object_value(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/bundles/export",
                body=encoded(command or BundleRequest()),
            )
        )

    async def install_bundle(
        self, namespace_id: str,  command: InstallBundle
    ) -> JsonObject:
        return object_value(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/bundles/install",
                body=encoded(command),
            )
        )

    async def admit_context(
        self, namespace_id: str,  command: ContextCommand
    ) -> ContextRecord:
        return ContextRecord.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/contexts",
                body=encoded(command),
            )
        )

    async def read_context(
        self, namespace_id: str,  context_id: str
    ) -> ContextRecord:
        return ContextRecord.model_validate(
            await self.request(
                "GET",
                self.namespace_path(namespace_id) + f"/contexts/{segment(context_id)}",
            )
        )

    async def context_feedback(
        self, namespace_id: str,  command: ContextFeedback
    ) -> JsonObject:
        return object_value(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/contexts/feedback",
                body=encoded(command),
            )
        )

    async def prepare_context(
        self, namespace_id: str,  command: ContextPrepare
    ) -> ContextPreparation:
        return ContextPreparation.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/contexts/preparations",
                body=encoded(command),
            )
        )

    async def context_preparation(
        self, namespace_id: str,  preparation_id: str
    ) -> ContextPreparation:
        return ContextPreparation.model_validate(
            await self.request(
                "GET",
                self.namespace_path(namespace_id)
                + f"/contexts/preparations/{segment(preparation_id)}",
            )
        )

    async def cancel_context_preparation(
        self, namespace_id: str,  preparation_id: str, operation_id: str
    ) -> ContextPreparation:
        return ContextPreparation.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id)
                + f"/contexts/preparations/{segment(preparation_id)}/cancel",
                body={"operation_id": operation_id},
            )
        )

    async def admit_managed_context(
        self, namespace_id: str,  command: ContextAdmissionCommand
    ) -> ContextAdmission:
        return ContextAdmission.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/contexts/admissions",
                body=encoded(command),
            )
        )

    async def report_managed_context(
        self, namespace_id: str,  command: ContextReport
    ) -> ContextReportResult:
        return ContextReportResult.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/contexts/reports",
                body=encoded(command),
            )
        )

    async def open_context_session(
        self, namespace_id: str,  command: OpenContextSession
    ) -> ContextSession:
        return ContextSession.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/contexts/sessions",
                body=encoded(command),
            )
        )

    async def close_context_session(
        self, namespace_id: str,  session_id: str, operation_id: str
    ) -> ContextSession:
        return ContextSession.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id)
                + f"/contexts/sessions/{segment(session_id)}/close",
                body={"operation_id": operation_id},
            )
        )

    async def submit_context_preparation(
        self, namespace_id: str,  command: ContextPrepare
    ) -> ContextPreparationJob:
        return ContextPreparationJob.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/contexts/preparation-jobs",
                body=encoded(command),
            )
        )

    async def context_session(
        self, namespace_id: str,  session_id: str
    ) -> ContextSession:
        return ContextSession.model_validate(
            await self.request(
                "GET",
                self.namespace_path(namespace_id)
                + f"/contexts/sessions/{segment(session_id)}",
            )
        )

    async def change_context_residents(
        self, namespace_id: str,  command: ChangeResidents
    ) -> ContextSession:
        return ContextSession.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/contexts/residents",
                body=encoded(command),
            )
        )

    async def change_work_context(
        self, namespace_id: str,  command: ChangeWorkContext
    ) -> WorkContext:
        return WorkContext.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/contexts/work",
                body=encoded(command),
            )
        )

    async def work_context(
        self, namespace_id: str,  work_context_id: str
    ) -> WorkContext:
        return WorkContext.model_validate(
            await self.request(
                "GET",
                self.namespace_path(namespace_id)
                + f"/contexts/work/{segment(work_context_id)}",
            )
        )

    async def change_context_session(
        self, namespace_id: str,  command: ChangeContextSession
    ) -> ContextSession:
        return ContextSession.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/contexts/sessions/change",
                body=encoded(command),
            )
        )

    async def work_contexts(
        self, namespace_id: str,  limit: int = 50
    ) -> tuple[WorkContext, ...]:
        return TypeAdapter(tuple[WorkContext, ...]).validate_python(
            await self.request(
                "GET",
                self.namespace_path(namespace_id) + "/contexts/work",
                params={"limit": limit},
            )
        )

    async def work_context_history(
        self, namespace_id: str,  work_context_id: str, limit: int = 50
    ) -> tuple[WorkContext, ...]:
        return TypeAdapter(tuple[WorkContext, ...]).validate_python(
            await self.request(
                "GET",
                self.namespace_path(namespace_id)
                + f"/contexts/work/{segment(work_context_id)}/history",
                params={"limit": limit},
            )
        )

    async def context_admission(
        self, namespace_id: str,  admission_id: str
    ) -> ContextAdmission:
        return ContextAdmission.model_validate(
            await self.request(
                "GET",
                self.namespace_path(namespace_id)
                + f"/contexts/admissions/{segment(admission_id)}",
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
