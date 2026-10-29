"""Typed operations over the service's domain contracts."""

from collections.abc import Mapping
from typing import TypeVar
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
    ContextPreparation,
    ContextPreparationJob,
    ContextPrepare,
    ContextReport,
    ContextReportResult,
    ContextSession,
    OpenContextSession,
    WorkContext,
)
from .contracts.lifecycle import MemoryChange
from .contracts.self_model import AdoptSeed, SeedResult
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
    async def adopt_seed(
        self, namespace_id: str, subject_id: str, command: AdoptSeed
    ) -> SeedResult:
        return SeedResult.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/self/seed",
                body=encoded(command),
            )
        )

    async def self_profile(self, namespace_id: str, subject_id: str) -> tuple[CognitiveRecord, ...]:
        return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
            await self.request("GET", self.subject_path(namespace_id, subject_id) + "/self/profile")
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
    def subject_path(namespace_id: str, subject_id: str) -> str:
        return f"/api/namespaces/{segment(namespace_id)}/subjects/{segment(subject_id)}"

    async def freeze_catalog(
        self, namespace_id: str, subject_id: str, command: FreezeCatalog
    ) -> CatalogView:
        return CatalogView.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/imports/catalogs",
                body=encoded(command),
            )
        )

    async def import_catalog(
        self, namespace_id: str, subject_id: str, catalog_id: str
    ) -> CatalogView:
        return CatalogView.model_validate(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id)
                + f"/imports/catalogs/{segment(catalog_id)}",
            )
        )

    async def submit_catalog(
        self, namespace_id: str, subject_id: str, command: ProcessCatalog
    ) -> ImportSubmission:
        return ImportSubmission.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/imports/process",
                body=encoded(command),
            )
        )

    async def admit_documents(
        self, namespace_id: str, subject_id: str, command: AdmitFiles
    ) -> ImportResult:
        return ImportResult.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/imports/files",
                body=encoded(command),
            )
        )

    async def import_unit_output(
        self, namespace_id: str, subject_id: str, output_id: str
    ) -> UnitOutput:
        return UnitOutput.model_validate(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id)
                + f"/imports/outputs/{segment(output_id)}",
            )
        )

    async def query_cognition(
        self, namespace_id: str, subject_id: str, command: QueryRequest
    ) -> QueryResult:
        return QueryResult.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/query",
                body=encoded(command),
            )
        )

    async def adopt_cognition(
        self, namespace_id: str, subject_id: str, command: AdoptCognition
    ) -> CognitiveRecord:
        return CognitiveRecord.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/cognition",
                body=encoded(command),
            )
        )

    async def cognitive_revision(
        self, namespace_id: str, subject_id: str, reference: CognitiveRef
    ) -> CognitiveRecord:
        return CognitiveRecord.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/cognition/read",
                body=encoded(reference),
            )
        )

    async def merge_concepts(
        self, namespace_id: str, subject_id: str, command: MergeConcepts
    ) -> tuple[CognitiveRecord, ...]:
        return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/concepts/merge",
                body=encoded(command),
            )
        )

    async def split_concept(
        self, namespace_id: str, subject_id: str, command: SplitConcept
    ) -> tuple[CognitiveRecord, ...]:
        return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/concepts/split",
                body=encoded(command),
            )
        )

    async def resegment_episodes(
        self, namespace_id: str, subject_id: str, command: ResegmentEpisodes
    ) -> tuple[CognitiveRecord, ...]:
        return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/episodes/resegment",
                body=encoded(command),
            )
        )

    async def source_directory(self, namespace_id: str, subject_id: str) -> tuple[Source, ...]:
        return TypeAdapter(tuple[Source, ...]).validate_python(
            await self.request("GET", self.subject_path(namespace_id, subject_id) + "/sources")
        )

    async def preview_sources(
        self, namespace_id: str, subject_id: str, sources: tuple[str, ...]
    ) -> SourcePreview:
        return SourcePreview.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/sources/preview",
                body={"observation_ids": list(sources)},
            )
        )

    async def trash_sources(
        self, namespace_id: str, subject_id: str, command: SourceCommand
    ) -> SourceResult:
        return SourceResult.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/sources/trash",
                body=encoded(command),
            )
        )

    async def restore_sources(
        self, namespace_id: str, subject_id: str, command: SourceCommand
    ) -> SourceResult:
        return SourceResult.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/sources/restore",
                body=encoded(command),
            )
        )

    async def purge_sources(
        self, namespace_id: str, subject_id: str, command: SourceCommand
    ) -> SourceResult:
        return SourceResult.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/sources/purge",
                body=encoded(command),
            )
        )

    async def export_bundle(
        self, namespace_id: str, subject_id: str, command: BundleRequest | None = None
    ) -> JsonObject:
        return object_value(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/bundles/export",
                body=encoded(command or BundleRequest()),
            )
        )

    async def install_bundle(
        self, namespace_id: str, subject_id: str, command: InstallBundle
    ) -> JsonObject:
        return object_value(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/bundles/install",
                body=encoded(command),
            )
        )

    async def admit_context(
        self, namespace_id: str, subject_id: str, command: ContextCommand
    ) -> ContextRecord:
        return ContextRecord.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/contexts",
                body=encoded(command),
            )
        )

    async def read_context(
        self, namespace_id: str, subject_id: str, context_id: str
    ) -> ContextRecord:
        return ContextRecord.model_validate(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id) + f"/contexts/{segment(context_id)}",
            )
        )

    async def context_feedback(
        self, namespace_id: str, subject_id: str, command: ContextFeedback
    ) -> JsonObject:
        return object_value(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/contexts/feedback",
                body=encoded(command),
            )
        )

    async def prepare_context(
        self, namespace_id: str, subject_id: str, command: ContextPrepare
    ) -> ContextPreparation:
        return ContextPreparation.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/contexts/preparations",
                body=encoded(command),
            )
        )

    async def context_preparation(
        self, namespace_id: str, subject_id: str, preparation_id: str
    ) -> ContextPreparation:
        return ContextPreparation.model_validate(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id)
                + f"/contexts/preparations/{segment(preparation_id)}",
            )
        )

    async def cancel_context_preparation(
        self, namespace_id: str, subject_id: str, preparation_id: str, operation_id: str
    ) -> ContextPreparation:
        return ContextPreparation.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id)
                + f"/contexts/preparations/{segment(preparation_id)}/cancel",
                body={"operation_id": operation_id},
            )
        )

    async def admit_managed_context(
        self, namespace_id: str, subject_id: str, command: ContextAdmissionCommand
    ) -> ContextAdmission:
        return ContextAdmission.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/contexts/admissions",
                body=encoded(command),
            )
        )

    async def report_managed_context(
        self, namespace_id: str, subject_id: str, command: ContextReport
    ) -> ContextReportResult:
        return ContextReportResult.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/contexts/reports",
                body=encoded(command),
            )
        )

    async def open_context_session(
        self, namespace_id: str, subject_id: str, command: OpenContextSession
    ) -> ContextSession:
        return ContextSession.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/contexts/sessions",
                body=encoded(command),
            )
        )

    async def close_context_session(
        self, namespace_id: str, subject_id: str, session_id: str, operation_id: str
    ) -> ContextSession:
        return ContextSession.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id)
                + f"/contexts/sessions/{segment(session_id)}/close",
                body={"operation_id": operation_id},
            )
        )

    async def submit_context_preparation(
        self, namespace_id: str, subject_id: str, command: ContextPrepare
    ) -> ContextPreparationJob:
        return ContextPreparationJob.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/contexts/preparation-jobs",
                body=encoded(command),
            )
        )

    async def context_session(
        self, namespace_id: str, subject_id: str, session_id: str
    ) -> ContextSession:
        return ContextSession.model_validate(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id)
                + f"/contexts/sessions/{segment(session_id)}",
            )
        )

    async def change_context_residents(
        self, namespace_id: str, subject_id: str, command: ChangeResidents
    ) -> ContextSession:
        return ContextSession.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/contexts/residents",
                body=encoded(command),
            )
        )

    async def change_work_context(
        self, namespace_id: str, subject_id: str, command: ChangeWorkContext
    ) -> WorkContext:
        return WorkContext.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/contexts/work",
                body=encoded(command),
            )
        )

    async def work_context(
        self, namespace_id: str, subject_id: str, work_context_id: str
    ) -> WorkContext:
        return WorkContext.model_validate(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id)
                + f"/contexts/work/{segment(work_context_id)}",
            )
        )

    async def change_context_session(
        self, namespace_id: str, subject_id: str, command: ChangeContextSession
    ) -> ContextSession:
        return ContextSession.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/contexts/sessions/change",
                body=encoded(command),
            )
        )

    async def work_contexts(
        self, namespace_id: str, subject_id: str, limit: int = 50
    ) -> tuple[WorkContext, ...]:
        return TypeAdapter(tuple[WorkContext, ...]).validate_python(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id) + "/contexts/work",
                params={"limit": limit},
            )
        )

    async def work_context_history(
        self, namespace_id: str, subject_id: str, work_context_id: str, limit: int = 50
    ) -> tuple[WorkContext, ...]:
        return TypeAdapter(tuple[WorkContext, ...]).validate_python(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id)
                + f"/contexts/work/{segment(work_context_id)}/history",
                params={"limit": limit},
            )
        )

    async def context_admission(
        self, namespace_id: str, subject_id: str, admission_id: str
    ) -> ContextAdmission:
        return ContextAdmission.model_validate(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id)
                + f"/contexts/admissions/{segment(admission_id)}",
            )
        )

    async def update_memory(
        self, namespace_id: str, subject_id: str, command: MemoryChange
    ) -> CognitiveRecord:
        return CognitiveRecord.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/memories/change",
                body=encoded(command),
            )
        )

    async def adopt_media(
        self, namespace_id: str, subject_id: str, command: AdoptMedia
    ) -> JsonObject:
        return object_value(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/media/adopt",
                body=encoded(command),
            )
        )
