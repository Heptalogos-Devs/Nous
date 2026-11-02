"""Source selection and operation history use the authoritative source lifecycle."""

from collections.abc import Mapping

from pydantic import JsonValue, TypeAdapter

from ..contracts import SourceCommand, SourceResult
from ..contracts.management import SourceSelection
from ..operations import object_value
from ..source_scope import SourceReadScope
from ..types import JsonObject
from .knowledge import KnowledgeClient
from .requests import operation_values, request_scope


class SourceClient(KnowledgeClient):
    async def source_operation(
        self,
        namespace_id: str,
        
        commit_seq: int | None = None,
        *,
        source_operation_id: str | None = None,
    ) -> JsonObject:
        if (commit_seq is None) == (source_operation_id is None):
            raise ValueError("Select a source operation identity or commit sequence")
        return object_value(
            await self.request(
                "GET",
                self.namespace_path(namespace_id) + "/sources/operation",
                params={"commit_seq": commit_seq, "source_operation_id": source_operation_id},
            )
        )

    async def source_operations(
        self,
        namespace_id: str,
        
        *,
        limit: int = 100,
        before_commit_seq: int | None = None,
    ) -> tuple[JsonObject, ...]:
        return TypeAdapter(tuple[JsonObject, ...]).validate_python(
            await self.request(
                "GET",
                self.namespace_path(namespace_id) + "/sources/operations",
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
        namespace = await request_scope(self, values)
        action = request.get("action")
        if action in {"get", "get_operation", "operation"}:
            return await self.source_operation(
                namespace, 
                TypeAdapter(int | None).validate_python(values.pop("commit_seq", None)),
                source_operation_id=TypeAdapter(str | None).validate_python(
                    values.pop("source_operation_id", None)
                ),
            )
        if action in {"list_operations", "operations"}:
            items = await self.source_operations(
                namespace, 
                limit=TypeAdapter(int).validate_python(values.pop("limit", 100)),
                before_commit_seq=TypeAdapter(int | None).validate_python(
                    values.pop("before_commit_seq", None)
                ),
            )
            return {"items": list(items)}
        if not isinstance(action, str):
            raise ValueError("Source operation requires action")
        if action == "list":
            return {
                "items": [
                    value.model_dump(mode="json")
                    for value in await self.source_directory(namespace)
                ]
            }
        mode = TypeAdapter(str | None).validate_python(values.pop("mode", "source"))
        if mode not in {None, "", "source"}:
            raise ValueError("Native source selections use canonical references or a RecordsQuery")
        values.pop("cascade", None)
        if "query" in values and "records" not in values:
            values["source_names"] = [values.pop("query")]
        if "request_id" in values:
            values["operation_id"] = values.pop("request_id")
        if "sources" in values:
            values["source_names"] = values.pop("sources")
        if "source" in values:
            values["source_names"] = [values.pop("source")]
        selected_operation = values.pop("source_operation_id", None)
        selected_commit = values.pop("trash_commit_seq", None)
        if selected_operation is not None or selected_commit is not None:
            operation = await self.source_operation(
                namespace, 
                TypeAdapter(int | None).validate_python(selected_commit),
                source_operation_id=TypeAdapter(str | None).validate_python(selected_operation),
            )
            selected = SourceResult.model_validate(operation["result"])
            values["observation_ids"], values["references"] = (
                list(selected.observation_ids),
                [reference.model_dump(mode="json") for reference in selected.references],
            )
        if action == "preview":
            return (
                await self.preview_sources(
                    namespace,  selection=SourceSelection.model_validate(values)
                )
            ).model_dump(mode="json")
        values.setdefault("reason", "Source lifecycle change")
        command = SourceCommand.model_validate(values)
        if action in {"trash", "delete", "execute", "batch_delete"}:
            return (await self.trash_sources(namespace,  command)).model_dump(mode="json")
        if action == "restore":
            return (await self.restore_sources(namespace,  command)).model_dump(mode="json")
        if action == "purge":
            return (await self.purge_sources(namespace,  command)).model_dump(mode="json")
        raise ValueError("Unknown native source operation")
