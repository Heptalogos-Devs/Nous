"""MaiBot-compatible tool facade over actual Nous domain operations."""

from collections.abc import Mapping, Sequence
from copy import copy
from pathlib import Path
from typing import Literal, Self
from uuid import uuid4

from pydantic import JsonValue

from ..client import NousClient
from ..contracts import ImportDocument
from ..contracts.context import ContextTarget
from ..files import ManagedFiles
from ..operations import encoded, object_value
from ..source_scope import SourceReadScope
from ..types import JsonObject
from .contracts import AMemorixOperation, AMemorixRequest
from .references import target_reference


class AMemorixAdapter:
    def __init__(
        self,
        client: NousClient,
        *,
        namespace_id: str,
        subject_id: str,
        wait_timeout_ms: int | None = None,
        transfer_root: str | Path | None = None,
    ) -> None:
        self.client, self.namespace_id, self.subject_id = client, namespace_id, subject_id
        self.wait_timeout_ms = wait_timeout_ms
        self.transfer_root = ManagedFiles(transfer_root).root if transfer_root is not None else None

    @property
    def managed_files(self) -> ManagedFiles:
        if self.transfer_root is None:
            raise ValueError("File transfer requires an explicit transfer_root")
        return ManagedFiles(self.transfer_root)

    def with_source_read(self, scope: SourceReadScope | Mapping[str, JsonValue]) -> Self:
        bound = copy(self)
        bound.client = self.client.with_source_read(scope)
        return bound

    async def upload_import_inputs(
        self,
        files: Sequence[str | Path | Mapping[str, JsonValue]],
        *,
        input_format: Literal["text", "json", "jsonl"] | None = None,
        operation_id: str | None = None,
    ) -> tuple[ImportDocument, ...]:
        from ..imports import upload_import_inputs

        return await upload_import_inputs(
            self.client,
            self.namespace_id,
            self.subject_id,
            files,
            transfer_root=self.managed_files.root,
            input_format=input_format,
            operation_id=operation_id,
        )

    async def call(self, tool: str, arguments: Mapping[str, JsonValue]) -> JsonObject:
        command = AMemorixRequest(
            namespace_id=self.namespace_id,
            subject_id=self.subject_id,
            operation=AMemorixOperation(tool=tool, arguments=dict(arguments)),
        )
        return object_value(
            await self.client.request("POST", "/api/integrations/amemorix", body=encoded(command))
        )

    async def search_memory(
        self,
        query: str = "",
        limit: int = 5,
        mode: str = "search",
        chat_id: str = "",
        person_id: str = "",
        time_start: str | float | None = None,
        time_end: str | float | None = None,
        respect_filter: bool = True,
        shared_chat_ids: Sequence[str] | None = None,
        **kwargs: JsonValue,
    ) -> JsonObject:
        return await self.call(
            "search_memory",
            {
                "query": query,
                "limit": limit,
                "mode": mode,
                "chat_id": chat_id,
                "person_id": person_id,
                "time_start": time_start,
                "time_end": time_end,
                "respect_filter": respect_filter,
                "shared_chat_ids": list(shared_chat_ids or ()),
                **kwargs,
            },
        )

    async def ingest_text(
        self,
        external_id: str,
        source_type: str,
        text: str,
        chat_id: str = "",
        person_ids: Sequence[str] | None = None,
        participants: Sequence[str] | None = None,
        timestamp: float | str | None = None,
        time_start: float | str | None = None,
        time_end: float | str | None = None,
        tags: Sequence[str] | None = None,
        metadata: Mapping[str, JsonValue] | None = None,
        respect_filter: bool = True,
        entities: Sequence[str] | None = None,
        relations: Sequence[Mapping[str, JsonValue]] | None = None,
        **kwargs: JsonValue,
    ) -> JsonObject:
        return await self.call(
            "ingest_text",
            {
                "external_id": external_id,
                "source_type": source_type,
                "text": text,
                "chat_id": chat_id,
                "person_ids": list(person_ids or ()),
                "participants": list(participants or ()),
                "timestamp": timestamp,
                "time_start": time_start,
                "time_end": time_end,
                "tags": list(tags or ()),
                "metadata": dict(metadata or {}),
                "respect_filter": respect_filter,
                "entities": list(entities or ()),
                "relations": [dict(value) for value in relations or ()],
                **kwargs,
            },
        )

    async def ingest_summary(
        self,
        external_id: str,
        chat_id: str,
        text: str,
        participants: Sequence[str] | None = None,
        time_start: float | str | None = None,
        time_end: float | str | None = None,
        tags: Sequence[str] | None = None,
        metadata: Mapping[str, JsonValue] | None = None,
        respect_filter: bool = True,
        **kwargs: JsonValue,
    ) -> JsonObject:
        return await self.call(
            "ingest_summary",
            {
                "external_id": external_id,
                "chat_id": chat_id,
                "text": text,
                "participants": list(participants or ()),
                "time_start": time_start,
                "time_end": time_end,
                "tags": list(tags or ()),
                "metadata": dict(metadata or {}),
                "respect_filter": respect_filter,
                **kwargs,
            },
        )

    async def maintain_memory(
        self,
        action: str,
        target: str = "",
        hours: float | None = None,
        reason: str = "",
        limit: int = 50,
        *,
        operation_id: str | None = None,
        **kwargs: JsonValue,
    ) -> JsonObject:
        return await self.call(
            "maintain_memory",
            {
                "action": action,
                "target": target,
                "hours": hours,
                "reason": reason,
                "limit": limit,
                "operation_id": operation_id or str(uuid4()),
                **kwargs,
            },
        )

    async def get_person_profile(
        self, person_id: str = "", chat_id: str = "", limit: int = 10, **kwargs: JsonValue
    ) -> JsonObject:
        return await self.call(
            "get_person_profile",
            {"person_id": person_id, "chat_id": chat_id, "limit": limit, **kwargs},
        )

    async def memory_source(
        self, target: str | Mapping[str, JsonValue] | ContextTarget
    ) -> JsonObject:
        if isinstance(target, str) and ":" not in target:
            target = (await self.client.memory(target)).reference.exact()
        return await self.call(
            "memory_source",
            {"target": target if isinstance(target, str) else encoded(target_reference(target))},
        )

    async def memory_stats(self, **kwargs: JsonValue) -> JsonObject:
        return await self.call("memory_stats", kwargs)

    async def memory_runtime_admin(self, *, action: str, **kwargs: JsonValue) -> JsonObject:
        return await self.call("memory_runtime_admin", {"action": action, **kwargs})

    async def memory_bundle_admin(self, *, action: str, **kwargs: JsonValue) -> JsonObject:
        from .bundles import bundle_admin

        return await bundle_admin(self, action=action, **kwargs)

    async def memory_source_admin(self, *, action: str, **kwargs: JsonValue) -> JsonObject:
        values = {"action": action, **kwargs}
        if action in {"delete", "batch_delete"}:
            values.setdefault("operation_id", str(uuid4()))
        return await self.call("memory_source_admin", values)

    async def memory_delete_admin(self, *, action: str, **kwargs: JsonValue) -> JsonObject:
        values = {"action": action, **kwargs}
        if action in {"execute", "restore", "purge"}:
            values.setdefault("request_id", str(uuid4()))
        return await self.call("memory_delete_admin", values)

    async def memory_import_admin(self, *, action: str, **kwargs: JsonValue) -> JsonObject:
        from .imports import import_admin

        return await import_admin(self, action=action, **kwargs)

    async def invoke(
        self,
        component_name: str,
        arguments: Mapping[str, JsonValue] | None = None,
        *,
        timeout_ms: int | None = None,
    ) -> JsonObject:
        values = dict(arguments or {})
        if component_name == "memory_profile_admin" and values.get("action") == "query":
            component_name = "get_person_profile"
            values.pop("action")
        if component_name == "memory_v5_admin":
            if values.get("action") == "remember_forever":
                values.update(action="protect", hours=0)
            if values.get("action") in {
                "reinforce",
                "weaken",
                "protect",
                "restore",
                "freeze",
                "forget",
                "recycle_bin",
                "unprotect",
                "trash",
                "purge",
                "decay",
            }:
                component_name = "maintain_memory"
                values.setdefault("operation_id", str(uuid4()))
        if component_name == "maintain_memory":
            values.setdefault("operation_id", str(uuid4()))
        if component_name == "memory_import_admin":
            action = values.pop("action", None)
            if not isinstance(action, str):
                raise ValueError("Import administration requires action")
            return await self.memory_import_admin(action=action, **values)
        if component_name == "memory_bundle_admin":
            action = values.pop("action", None)
            if not isinstance(action, str):
                raise ValueError("Bundle administration requires action")
            return await self.memory_bundle_admin(action=action, **values)
        if timeout_ms is not None:
            import anyio

            with anyio.fail_after(timeout_ms / 1000):
                return await self.call(component_name, values)
        return await self.call(component_name, values)
