"""Source selection and operation history use the authoritative source lifecycle."""

from collections.abc import Mapping

from pydantic import JsonValue, TypeAdapter

from ..operations import object_value
from ..source_scope import SourceReadScope
from ..types import JsonObject
from .knowledge import KnowledgeClient
from .requests import operation_values, request_scope


class SourceClient(KnowledgeClient):
    async def source_operation(
        self,
        namespace_id: str,
        subject_id: str,
        commit_seq: int | None = None,
        *,
        source_operation_id: str | None = None,
    ) -> JsonObject:
        if (commit_seq is None) == (source_operation_id is None):
            raise ValueError("Select a source operation identity or commit sequence")
        return object_value(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id) + "/sources/operation",
                params={"commit_seq": commit_seq, "source_operation_id": source_operation_id},
            )
        )

    async def source_operations(
        self,
        namespace_id: str,
        subject_id: str,
        *,
        limit: int = 100,
        before_commit_seq: int | None = None,
    ) -> tuple[JsonObject, ...]:
        return TypeAdapter(tuple[JsonObject, ...]).validate_python(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id) + "/sources/operations",
                params={"limit": limit, "before_commit_seq": before_commit_seq},
            )
        )

    async def sources(self, request: Mapping[str, JsonValue]) -> JsonObject:
        values = operation_values(request)
        source_read = values.pop("source_read", None)
        if source_read is not None:
            return await self.with_source_read(SourceReadScope.model_validate(source_read)).sources(
                {"action": request.get("action"), "request": values}
            )
        namespace, subject = await request_scope(self, values)
        action = request.get("action")
        if action in {"get", "get_operation", "operation"}:
            return await self.source_operation(
                namespace,
                subject,
                TypeAdapter(int | None).validate_python(values.pop("commit_seq", None)),
                source_operation_id=TypeAdapter(str | None).validate_python(
                    values.pop("source_operation_id", None)
                ),
            )
        if action in {"list_operations", "operations"}:
            items = await self.source_operations(
                namespace,
                subject,
                limit=TypeAdapter(int).validate_python(values.pop("limit", 100)),
                before_commit_seq=TypeAdapter(int | None).validate_python(
                    values.pop("before_commit_seq", None)
                ),
            )
            return {"items": list(items)}
        if not isinstance(action, str):
            raise ValueError("Source operation requires action")
        tool = (
            "memory_source_admin"
            if action in {"list", "delete", "batch_delete"}
            else "memory_delete_admin"
        )
        if action == "trash":
            action = "execute"
        if action == "restore" and "trash_commit_seq" in values:
            commit_seq = TypeAdapter(int).validate_python(values.pop("trash_commit_seq"))
            request_id = values.pop("operation_id", None)
            values["request_id"] = request_id
            values["operation_id"] = str(commit_seq)
        return object_value(
            await self.request(
                "POST",
                "/api/integrations/amemorix",
                body={
                    "namespace_id": namespace,
                    "subject_id": subject,
                    "operation": {"tool": tool, "arguments": {"action": action, **values}},
                },
            )
        )
