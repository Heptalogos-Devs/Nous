from collections.abc import Mapping, Sequence
from copy import copy
from pathlib import Path
from typing import Any
from uuid import uuid4

from ..client import NousClient
from ..errors import NousError
from ..files import ManagedFiles
from .bundles import bundle_admin
from .imports import import_admin
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
        self.client = client
        self.namespace_id = namespace_id
        self.subject_id = subject_id
        self.wait_timeout_ms = wait_timeout_ms
        self.transfer_root = ManagedFiles(transfer_root).root if transfer_root is not None else None

    @property
    def managed_files(self) -> ManagedFiles:
        if self.transfer_root is None:
            raise ValueError("File transfer requires an explicit transfer_root")
        return ManagedFiles(self.transfer_root)

    async def upload_import_inputs(
        self, files: Sequence[str | Path | Mapping[str, Any]], *, input_format: str | None = None,
        operation_id: str | None = None,
    ) -> list[dict[str, Any]]:
        from ..imports import upload_import_inputs
        return await upload_import_inputs(
            self.client, self.namespace_id, self.subject_id, files,
            transfer_root=self.managed_files.root, input_format=input_format, operation_id=operation_id,
        )

    def with_source_read(self, scope: Mapping) -> "AMemorixAdapter":
        bound = copy(self)
        bound.client = self.client.with_source_read(scope)
        return bound

    async def call(self, tool: str, arguments: Mapping[str, Any]) -> dict:
        result = await self.client.request(
            "POST",
            "/api/integrations/amemorix",
            body={
                "namespace_id": self.namespace_id,
                "subject_id": self.subject_id,
                "operation": {"tool": tool, "arguments": dict(arguments)},
            },
        )
        if not isinstance(result, dict):
            raise NousError(
                {
                    "code": "Internal",
                    "message": "A-Memorix operation returned a non-object",
                }
            )
        if result.get("accepted") and result.get("job_id"):
            options = (
                {}
                if self.wait_timeout_ms is None
                else {"timeout_ms": self.wait_timeout_ms}
            )
            jobs = await self.client.wait_success([result["job_id"]], **options)
            result = jobs[0].get("result")
            if not isinstance(result, dict):
                raise NousError(
                    {
                        "code": "Internal",
                        "message": "A-Memorix task has no result object",
                    }
                )
        return result

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
        **kwargs: Any,
    ) -> dict:
        return await self.call(
            "search_memory",
            dict(
                query=query,
                limit=limit,
                mode=mode,
                chat_id=chat_id,
                person_id=person_id,
                time_start=time_start,
                time_end=time_end,
                respect_filter=respect_filter,
                shared_chat_ids=list(shared_chat_ids or ()),
                **kwargs,
            ),
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
        metadata: Mapping | None = None,
        respect_filter: bool = True,
        entities: Sequence[str] | None = None,
        relations: Sequence[Mapping] | None = None,
        **kwargs: Any,
    ) -> dict:
        return await self.call(
            "ingest_text",
            dict(
                external_id=external_id,
                source_type=source_type,
                text=text,
                chat_id=chat_id,
                person_ids=list(person_ids or ()),
                participants=list(participants or ()),
                timestamp=timestamp,
                time_start=time_start,
                time_end=time_end,
                tags=list(tags or ()),
                metadata=dict(metadata or {}),
                respect_filter=respect_filter,
                entities=list(entities or ()),
                relations=[dict(item) for item in (relations or ())],
                **kwargs,
            ),
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
        metadata: Mapping | None = None,
        respect_filter: bool = True,
        **kwargs: Any,
    ) -> dict:
        return await self.call(
            "ingest_summary",
            dict(
                external_id=external_id,
                chat_id=chat_id,
                text=text,
                participants=list(participants or ()),
                time_start=time_start,
                time_end=time_end,
                tags=list(tags or ()),
                metadata=dict(metadata or {}),
                respect_filter=respect_filter,
                **kwargs,
            ),
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
        **kwargs: Any,
    ) -> dict:
        return await self.call(
            "maintain_memory",
            dict(
                action=action,
                target=target,
                hours=hours,
                reason=reason,
                limit=limit,
                operation_id=operation_id or str(uuid4()),
                **kwargs,
            ),
        )

    async def get_person_profile(
        self, person_id: str = "", chat_id: str = "", limit: int = 10, **kwargs: Any
    ) -> dict:
        return await self.call(
            "get_person_profile",
            dict(person_id=person_id, chat_id=chat_id, limit=limit, **kwargs),
        )

    async def memory_source(self, target: str | Mapping) -> dict:
        if isinstance(target, str) and ":" not in target:
            memory = await self.client.request("GET", f"/api/memories/{target}")
            target = f"memory:{memory['id']}@{memory['revision']}"
        return await self.call("memory_source", {"target": target_reference(target)})

    async def memory_stats(self, **kwargs: Any) -> dict:
        return await self.call("memory_stats", kwargs)

    async def memory_runtime_admin(self, *, action: str, **kwargs: Any) -> dict:
        return await self.call("memory_runtime_admin", {"action": action, **kwargs})

    async def memory_bundle_admin(self, *, action: str, **kwargs: Any) -> dict:
        return await bundle_admin(self, action=action, **kwargs)

    async def memory_source_admin(self, *, action: str, **kwargs: Any) -> dict:
        arguments = {"action": action, **kwargs}
        if action.strip().lower() in {"delete", "batch_delete"} and not arguments.get("operation_id"):
            arguments["operation_id"] = str(uuid4())
        return await self.call("memory_source_admin", arguments)

    async def memory_delete_admin(self, *, action: str, **kwargs: Any) -> dict:
        arguments = {"action": action, **kwargs}
        if action.strip().lower() in {"execute", "restore", "purge"} and not arguments.get("request_id"):
            arguments["request_id"] = str(uuid4())
        return await self.call("memory_delete_admin", arguments)

    async def memory_import_admin(self, *, action: str, **kwargs: Any) -> dict:
        return await import_admin(self, action=action, **kwargs)

    async def invoke(
        self,
        component_name: str,
        arguments: Mapping | None = None,
        *,
        timeout_ms: int | None = None,
    ) -> dict:
        adapter = self
        if timeout_ms is not None:
            adapter = copy(self)
            adapter.wait_timeout_ms = timeout_ms
        values = dict(arguments or {})
        if component_name == "memory_profile_admin" and values.get("action") == "query":
            values.pop("action")
            return await adapter.get_person_profile(**values)
        if component_name == "memory_v5_admin":
            action = values.get("action")
            if action == "remember_forever":
                values.update(action="protect", hours=0)
            if values.get("action") in {
                "reinforce",
                "weaken",
                "protect",
                "restore",
                "freeze",
                "forget",
                "recycle_bin",
            }:
                return await adapter.maintain_memory(**values)
        functions = {
            "memory_source_admin": adapter.memory_source_admin,
            "memory_delete_admin": adapter.memory_delete_admin,
            "memory_import_admin": adapter.memory_import_admin,
            "memory_stats": adapter.memory_stats,
            "memory_runtime_admin": adapter.memory_runtime_admin,
            "memory_bundle_admin": adapter.memory_bundle_admin,
            "search_memory": adapter.search_memory,
            "ingest_text": adapter.ingest_text,
            "ingest_summary": adapter.ingest_summary,
            "maintain_memory": adapter.maintain_memory,
            "get_person_profile": adapter.get_person_profile,
            "memory_source": adapter.memory_source,
        }
        function = functions.get(component_name)
        if function is None:
            raise NousError(
                {
                    "code": "CapabilityUnavailable",
                    "message": "This A-Memorix operation requires its domain service",
                    "context": {"tool": component_name, "action": values.get("action")},
                }
            )
        return await function(**values)
