"""Typed knowledge client operations."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import TYPE_CHECKING, Literal

from pydantic import JsonValue

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
from ..types import (
    JsonObject,
)

if TYPE_CHECKING:
    from ..contracts.lifecycle import MemoryChange


from .imports import ImportClient


class KnowledgeClient(ImportClient):
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
