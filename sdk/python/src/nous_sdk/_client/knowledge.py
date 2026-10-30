"""Typed knowledge client operations."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import TYPE_CHECKING, Literal

from pydantic import JsonValue, TypeAdapter

from ..contracts import (
    CognitiveRecord,
    CognitiveRef,
    MediaReference,
    QueryRequest,
    QueryResult,
    TimeFilter,
)
from ..contracts.references import ReferenceResolution
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
    async def reconcile_references(
        self, namespace_id: str, *, subject_id: str | None = None, operation_id: str | None = None
    ) -> dict[str, int]:
        from uuid import uuid4

        return TypeAdapter(dict[str, int]).validate_python(
            await self.request(
                "POST",
                f"/api/namespaces/{segment(namespace_id)}/references/reconcile",
                params={"subject_id": subject_id or self.subject_id},
                body={"operation_id": operation_id or str(uuid4())},
            )
        )

    async def cognitive_object(
        self, object_id: str, *, revision: int | None = None
    ) -> CognitiveRecord:
        return CognitiveRecord.model_validate(
            await self.request(
                "GET", f"/api/cognition/{segment(object_id)}", params={"revision": revision}
            )
        )

    async def cognitive_history(
        self, object_id: str, *, limit: int = 100
    ) -> tuple[CognitiveRecord, ...]:
        return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
            await self.request(
                "GET", f"/api/cognition/{segment(object_id)}/history", params={"limit": limit}
            )
        )

    async def concept_by_alias(
        self, namespace_id: str, subject_id: str, alias: str
    ) -> CognitiveRecord:
        return CognitiveRecord.model_validate(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id) + "/concepts/resolve",
                params={"alias": alias},
            )
        )

    async def set_cognitive_deleted(
        self,
        namespace_id: str,
        subject_id: str,
        reference: CognitiveRef,
        deleted: bool,
        *,
        operation_id: str | None = None,
    ) -> CognitiveRecord:
        from uuid import uuid4

        return CognitiveRecord.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/cognition/deleted",
                body={
                    "operation_id": operation_id or str(uuid4()),
                    "reference": encoded(reference),
                    "deleted": deleted,
                },
            )
        )

    async def query(
        self,
        subject_id: str,
        text: str = "",
        *,
        namespace_id: str | None = None,
        mode: Literal["lexical", "dense", "hybrid", "vcp"] = "lexical",
        kinds: Sequence[str] = (),
        domains: Sequence[str] = ("cognition",),
        source_ids: Sequence[str] | None = None,
        media: Sequence[MediaReference] = (),
        parts: Sequence[str | MediaReference] = (),
        times: Sequence[TimeFilter] = (),
        source_type: str | None = None,
        min_commit_seq: int | None = None,
        include_deleted: bool = False,
        include_inactive: bool = False,
        reference: str | None = None,
        history: bool = False,
        as_of_commit: int | None = None,
        limit: int = 10,
    ) -> QueryResult:
        values: JsonObject = {
            "text": text,
            "mode": mode,
            "kinds": list(kinds),
            "domains": list(domains),
            "source_ids": list(source_ids) if source_ids is not None else None,
            "reference": reference,
            "history": history,
            "as_of_commit": as_of_commit,
            "limit": limit,
            "media": [encoded(value) for value in media],
            "parts": [value if isinstance(value, str) else encoded(value) for value in parts],
            "times": [encoded(value) for value in times],
            "source_type": source_type,
            "min_commit_seq": min_commit_seq,
            "include_deleted": include_deleted,
            "include_inactive": include_inactive,
        }
        command = QueryRequest.model_validate(values)
        namespace = namespace_id or self.namespace_id
        if namespace is not None:
            return await self.query_cognition(namespace, subject_id, command)
        return QueryResult.model_validate(
            await self.request(
                "POST", f"/api/subjects/{segment(subject_id)}/query", body=encoded(command)
            )
        )

    async def memory(self, memory_id: str) -> CognitiveRecord:
        return CognitiveRecord.model_validate(
            await self.request("GET", f"/api/memories/{segment(memory_id)}")
        )

    async def query_manual(self) -> str:
        value = await self.request("GET", "/api/query/manual")
        if not isinstance(value, str):
            raise NousError({"code": "invalid_response", "message": "Query manual is not text"})
        return value

    async def source(
        self,
        subject_id: str,
        reference: CognitiveRef | MediaReference | str,
        *,
        observation_id: str | None = None,
    ) -> JsonObject:
        return object_value(
            await self.request(
                "POST",
                "/api/sources/read",
                body={
                    "subject_id": subject_id,
                    "reference": reference if isinstance(reference, str) else encoded(reference),
                    "observation_id": observation_id,
                },
            )
        )

    async def memory_source(self, memory_id: str, *, revision: int | None = None) -> JsonObject:
        return object_value(
            await self.request(
                "GET", f"/api/memories/{segment(memory_id)}/source", params={"revision": revision}
            )
        )

    async def resolve_reference(
        self,
        subject_id: str,
        reference: CognitiveRef | MediaReference | str,
        *,
        as_of_commit: int | None = None,
    ) -> ReferenceResolution:
        return ReferenceResolution.model_validate(
            await self.request(
                "POST",
                "/api/references",
                body={
                    "subject_id": subject_id,
                    "reference": reference if isinstance(reference, str) else encoded(reference),
                    "as_of_commit": as_of_commit,
                },
            )
        )

    async def change_memories(
        self, namespace_id: str, subject_id: str, command: MemoryChanges
    ) -> tuple[CognitiveRecord, ...]:
        return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/memories/batch",
                body=encoded(command),
            )
        )

    async def change_memory(
        self, memory_id: str, request: Mapping[str, JsonValue] | MemoryChange
    ) -> CognitiveRecord:
        from ..contracts.lifecycle import MemoryChange

        current = await self.memory(memory_id)
        command = (
            request
            if isinstance(request, MemoryChange)
            else MemoryChange.model_validate({"reference": encoded(current.reference), **request})
        )
        if command.reference.id != memory_id:
            raise ValueError("Memory command targets a different object")
        return await self.update_memory(current.namespace_id, current.subject_id, command)

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
        if action in {"get", "history", "source", "set_deleted"}:
            object_id = required_text(values, "id")
            if action == "history":
                return [
                    encoded(record)
                    for record in await self.cognitive_history(
                        object_id, limit=TypeAdapter(int).validate_python(values.pop("limit", 100))
                    )
                ]
            revision = TypeAdapter(int | None).validate_python(values.pop("revision", None))
            if action == "set_deleted":
                revision = TypeAdapter(int).validate_python(values.pop("expected_revision"))
            record = await self.cognitive_object(object_id, revision=revision)
            if action == "get":
                return encoded(record)
            if action == "source":
                return await self.source(record.subject_id, record.reference)
            return encoded(
                await self.set_cognitive_deleted(
                    record.namespace_id,
                    record.subject_id,
                    record.reference,
                    TypeAdapter(bool).validate_python(values.pop("deleted")),
                    operation_id=operation_identity(values),
                )
            )
        if action == "formation_run":
            return encoded(
                await self.job(
                    required_text(values, "job_id"),
                    namespace_id=TypeAdapter(str | None).validate_python(
                        values.pop("namespace_id", self.namespace_id)
                    ),
                    subject_id=TypeAdapter(str | None).validate_python(
                        values.pop("subject_id", self.subject_id)
                    ),
                )
            )
        if action == "revise" and not values.get("subject_id") and self.subject_id is None:
            current = await self.cognitive_object(
                TypeAdapter(str).validate_python(values.get("id"))
            )
            values.update({"namespace_id": current.namespace_id, "subject_id": current.subject_id})
        namespace, subject = await request_scope(self, values)
        if action in {"create", "revise", "adopt"}:
            values["operation_id"] = operation_identity(values)
            return encoded(
                await self.adopt_cognition(
                    namespace, subject, AdoptCognition.model_validate(values)
                )
            )
        if action == "concept_resolve":
            return encoded(
                await self.concept_by_alias(namespace, subject, required_text(values, "alias"))
            )
        if action == "form":
            return encoded(
                await self.form_memory(
                    subject,
                    required_text(values, "observation_id"),
                    source=TypeAdapter(MediaReference | None).validate_python(
                        values.pop("source", None)
                    ),
                    operation_id=operation_identity(values),
                )
            )
        if action == "query":
            return encoded(
                await self.query_cognition(namespace, subject, QueryRequest.model_validate(values))
            )
        values["operation_id"] = operation_identity(values)
        if action == "merge_concepts":
            return [
                encoded(record)
                for record in await self.merge_concepts(
                    namespace, subject, MergeConcepts.model_validate(values)
                )
            ]
        if action == "split_concept":
            return [
                encoded(record)
                for record in await self.split_concept(
                    namespace, subject, SplitConcept.model_validate(values)
                )
            ]
        if action == "resegment_episodes":
            return [
                encoded(record)
                for record in await self.resegment_episodes(
                    namespace, subject, ResegmentEpisodes.model_validate(values)
                )
            ]
        raise ValueError("Unknown cognition action")
