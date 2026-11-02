"""Typed knowledge client operations."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import TYPE_CHECKING, Literal

from pydantic import JsonValue, TypeAdapter

from ..contracts import (
    CognitiveRecord,
    CognitiveRef,
    MediaReference,
    ReadRecords,
    RecordDetail,
    RecordPurpose,
    RecordsPage,
    RecordsQuery,
    TimeFilter,
)
from ..contracts.corrections import (
    ApplyCorrection,
    CorrectionPlan,
    CorrectionPlansPage,
    CorrectionPlansQuery,
    CorrectionResult,
    PlanCorrection,
    RollbackCorrection,
)
from ..contracts.references import ExactReference, ReferenceResolution
from ..contracts.resources import (
    ChangeResource,
    ReadResource,
    RegisterResource,
    ResourcePage,
    ResourceRecord,
)
from ..errors import NousError
from ..operations import encoded, object_value, segment
from ..source_scope import SourceReadScope
from ..types import (
    JsonObject,
)

if TYPE_CHECKING:
    from ..contracts.lifecycle import MemoryChange, MemoryChanges


from .imports import ImportClient


class KnowledgeClient(ImportClient):
    async def register_resource(
        self, namespace_id: str,  command: RegisterResource
    ) -> ResourceRecord:
        return ResourceRecord.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/resources",
                body=encoded(command),
            )
        )

    async def resources(
        self, namespace_id: str,  *, cursor: str | None = None, limit: int = 100
    ) -> ResourcePage:
        return ResourcePage.model_validate(
            await self.request(
                "GET",
                self.namespace_path(namespace_id) + "/resources",
                params={"cursor": cursor, "limit": limit},
            )
        )

    async def resource(
        self, namespace_id: str,  resource_id: str, *, revision: int | None = None
    ) -> ResourceRecord:
        return ResourceRecord.model_validate(
            await self.request(
                "GET",
                self.namespace_path(namespace_id) + "/resources/" + segment(resource_id),
                params={"revision": revision},
            )
        )

    async def read_resource(
        self, namespace_id: str,  command: ReadResource
    ) -> ResourceRecord:
        return ResourceRecord.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/resources/read",
                body=encoded(command),
            )
        )

    async def change_resource(
        self, namespace_id: str,  command: ChangeResource
    ) -> ResourceRecord:
        return ResourceRecord.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/resources/change",
                body=encoded(command),
            )
        )

    async def plan_correction(
        self, namespace_id: str,  command: PlanCorrection
    ) -> CorrectionPlan:
        return CorrectionPlan.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/corrections/plan",
                body=encoded(command),
            )
        )

    async def apply_correction(
        self, namespace_id: str,  command: ApplyCorrection
    ) -> CorrectionResult:
        return CorrectionResult.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/corrections/apply",
                body=encoded(command),
            )
        )

    async def rollback_correction(
        self, namespace_id: str,  command: RollbackCorrection
    ) -> CorrectionResult:
        return CorrectionResult.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/corrections/rollback",
                body=encoded(command),
            )
        )

    async def correction_plan(
        self, namespace_id: str,  plan_id: str
    ) -> CorrectionPlan:
        return CorrectionPlan.model_validate(
            await self.request(
                "GET",
                self.namespace_path(namespace_id) + "/corrections/" + segment(plan_id),
            )
        )

    async def correction_plans(
        self, namespace_id: str,  query: CorrectionPlansQuery | None = None
    ) -> CorrectionPlansPage:
        query = query or CorrectionPlansQuery()
        return CorrectionPlansPage.model_validate(
            await self.request(
                "GET",
                self.namespace_path(namespace_id) + "/corrections",
                params={"cursor": query.cursor, "limit": query.limit},
            )
        )

    async def records(
        self, namespace_id: str,  command: RecordsQuery
    ) -> RecordsPage:
        return RecordsPage.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/records/query",
                body=encoded(command),
            )
        )

    async def read_records(
        self, namespace_id: str,  command: ReadRecords
    ) -> tuple[CognitiveRecord, ...]:
        return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/records/read",
                body=encoded(command),
            )
        )

    async def record_detail(
        self,
        namespace_id: str,
        
        reference: CognitiveRef,
        *,
        history_limit: int = 100,
        purpose: RecordPurpose = "exact",
    ) -> RecordDetail:
        return RecordDetail.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/records/detail",
                body={
                    "reference": encoded(reference),
                    "history_limit": history_limit,
                    "purpose": purpose,
                },
            )
        )

    async def reconcile_references(
        self, namespace_id: str, *, operation_id: str | None = None
    ) -> dict[str, int]:
        from uuid import uuid4

        return TypeAdapter(dict[str, int]).validate_python(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/references/reconcile",
                params={},
                body={"operation_id": operation_id or str(uuid4())},
            )
        )

    async def concept_by_alias(
        self, namespace_id: str,  alias: str
    ) -> CognitiveRecord:
        return CognitiveRecord.model_validate(
            await self.request(
                "GET",
                self.namespace_path(namespace_id) + "/concepts/resolve",
                params={"alias": alias},
            )
        )

    async def query_manual(self) -> str:
        value = await self.request("GET", "/api/knowledge/query/manual")
        if not isinstance(value, str):
            raise NousError({"code": "invalid_response", "message": "Query manual is not text"})
        return value

    async def resolve_reference(
        self,
        namespace_id: str,
        reference: ExactReference | str,
        *,
        as_of_commit: int | None = None,
    ) -> ReferenceResolution:
        return ReferenceResolution.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/references",
                body={
                    
                    "reference": reference if isinstance(reference, str) else encoded(reference),
                    "as_of_commit": as_of_commit,
                },
            )
        )

    async def change_memories(
        self, namespace_id: str,  command: MemoryChanges
    ) -> tuple[CognitiveRecord, ...]:
        return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/memories/batch",
                body=encoded(command),
            )
        )

    async def cognition(self, request: Mapping[str, JsonValue]) -> JsonValue:
        from ..contracts import AdoptCognition, MergeConcepts, ResegmentEpisodes, SplitConcept
        from .requests import operation_identity, operation_values, request_scope, required_text

        values = operation_values(request)
        source_read = values.pop("source_read", None)
        if source_read is not None:
            return await self.with_source_read(
                SourceReadScope.model_validate(source_read)
            ).cognition({"action": request.get("action"), "request": values})
        action = request.get("action")
        if action == "formation_run":
            return encoded(
                await self.job(
                    required_text(values, "job_id"),
                    namespace_id=TypeAdapter(str | None).validate_python(
                        values.pop("namespace_id", self.namespace_id)
                    ),
                    
                )
            )
        namespace = await request_scope(self, values)
        if action in {"create", "revise", "adopt"}:
            values["operation_id"] = operation_identity(values)
            return encoded(
                await self.adopt_cognition(
                    namespace,  AdoptCognition.model_validate(values)
                )
            )
        if action == "concept_resolve":
            return encoded(
                await self.concept_by_alias(namespace,  required_text(values, "alias"))
            )
        if action == "form_observation":
            from ..contracts.imports import FormationPurpose

            return encoded(
                await self.form_observation(
                    namespace,
                    required_text(values, "observation_id"),
                    formation_purpose=TypeAdapter(FormationPurpose).validate_python(
                        values.pop("formation_purpose", "memory")
                    ),
                    background=TypeAdapter(str).validate_python(
                        values.pop("background", "")
                    ),
                    source=TypeAdapter(MediaReference | None).validate_python(
                        values.pop("source", None)
                    ),
                    operation_id=operation_identity(values),
                )
            )
        values["operation_id"] = operation_identity(values)
        if action == "merge_concepts":
            return [
                encoded(record)
                for record in await self.merge_concepts(
                    namespace,  MergeConcepts.model_validate(values)
                )
            ]
        if action == "split_concept":
            return [
                encoded(record)
                for record in await self.split_concept(
                    namespace,  SplitConcept.model_validate(values)
                )
            ]
        if action == "resegment_episodes":
            return [
                encoded(record)
                for record in await self.resegment_episodes(
                    namespace,  ResegmentEpisodes.model_validate(values)
                )
            ]
        raise ValueError("Unknown cognition action")
