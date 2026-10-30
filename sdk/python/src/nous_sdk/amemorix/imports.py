from typing import TYPE_CHECKING, Literal
from uuid import uuid4

from pydantic import JsonValue, TypeAdapter

from ..imports import upload_import_file_documents
from ..operations import encoded, object_value
from ..types import JsonObject

if TYPE_CHECKING:
    from .client import AMemorixAdapter


async def import_admin(
    adapter: "AMemorixAdapter", *, action: str, **kwargs: JsonValue
) -> JsonObject:
    action = action.strip().lower()
    if action in {"path_aliases", "get_path_aliases"}:
        if kwargs:
            raise ValueError("path_aliases accepts no parameters")
        return {"success": True, "path_aliases": dict(adapter.managed_files.path_aliases())}
    if action in {"resolve", "resolve_path"}:
        alias = kwargs.get("alias", "raw")
        relative = kwargs.get("relative_path", "")
        if not isinstance(alias, str) or not isinstance(relative, str):
            raise ValueError("alias and relative_path must be text")
        return object_value(encoded(adapter.managed_files.resolve_path(alias, relative)))
    values = dict(kwargs)
    input_format = TypeAdapter(Literal["text", "json", "jsonl"] | None).validate_python(
        values.get("input_mode")
    )
    if action in {"create_upload", "create_raw_scan", "create_paste", "cancel", "retry_failed"}:
        values.setdefault("operation_id", str(uuid4()))
    if action in {"create_upload", "create_raw_scan"}:
        operation = values["operation_id"]
        assert isinstance(operation, str)
        if action == "create_upload":
            files = TypeAdapter(list[str | JsonObject]).validate_python(
                values.pop("staged_files", None)
                or values.pop("files", None)
                or values.pop("uploads", None)
                or []
            )
            if files:
                documents = await upload_import_file_documents(
                    adapter.client,
                    adapter.namespace_id,
                    adapter.subject_id,
                    files,
                    transfer_root=adapter.managed_files.root,
                    operation_id=operation,
                    input_format=input_format,
                )
                values["inputs"] = [encoded(document) for document in documents]
        else:
            alias = values.pop("alias", "raw")
            relative = values.pop("relative_path", "")
            pattern = values.pop("glob", "*")
            recursive = values.pop("recursive", True)
            if (
                not isinstance(alias, str)
                or alias != "raw"
                or not isinstance(relative, str)
                or not isinstance(pattern, str)
                or not isinstance(recursive, bool)
            ):
                raise ValueError("Raw scan requires the raw alias and valid scan options")
            files = await adapter.managed_files.scan(
                alias, relative, pattern=pattern, recursive=recursive
            )
            documents = await upload_import_file_documents(
                adapter.client,
                adapter.namespace_id,
                adapter.subject_id,
                files,
                transfer_root=adapter.managed_files.root,
                operation_id=operation,
                alias=alias,
                input_format=input_format,
            )
            values["inputs"] = [encoded(document) for document in documents]
    return await adapter.call("memory_import_admin", {"action": action, **values})
