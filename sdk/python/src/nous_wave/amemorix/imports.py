from pathlib import Path
from typing import TYPE_CHECKING, Any
from uuid import uuid4

from ..errors import NousError

if TYPE_CHECKING:
    from .client import AMemorixAdapter


async def import_admin(adapter: "AMemorixAdapter", *, action: str, **kwargs: Any) -> dict:
    action = action.strip().lower()
    if action in {"path_aliases", "get_path_aliases"}:
        if kwargs:
            raise ValueError("path_aliases accepts no parameters")
        return {"success": True, "path_aliases": adapter.managed_files.path_aliases()}
    if action in {"resolve", "resolve_path"}:
        return adapter.managed_files.resolve_path(**kwargs)
    values = dict(kwargs)
    if action in {"create_upload", "create_raw_scan", "create_paste", "cancel", "retry_failed"}:
        values.setdefault("operation_id", str(uuid4()))
    if action == "create_upload":
        files = values.pop("staged_files", None) or values.pop("files", None) or values.pop("uploads", None)
        if files is not None:
            if "inputs" in values:
                raise ValueError("Supply staged files or uploaded inputs, not both")
            prepared = []
            for value in files:
                item = dict(value) if isinstance(value, dict) else {"path": value}
                name = item.get("filename") or item.get("name") or Path(item.get("staged_path") or item.get("path")).name
                if Path(name).suffix.lower() not in {".txt", ".md", ".json"}:
                    raise NousError({"code": "CapabilityUnavailable", "message": "A-Memorix upload accepts txt/md/json; other formats use their native parser"})
                if Path(name).suffix.lower() == ".json":
                    item["format"] = "json"
                prepared.append(item)
            values["inputs"] = await adapter.upload_import_inputs(
                prepared, input_format=values.get("input_mode", "text"), operation_id=values["operation_id"],
            )
    elif action == "create_raw_scan":
        alias = values.pop("alias", "raw")
        if alias != "raw":
            raise ValueError("raw scan requires the raw alias")
        files = await adapter.managed_files.scan(
            alias, values.pop("relative_path", ""), pattern=values.pop("glob", "*"), recursive=values.pop("recursive", True),
        )
        files = [path for path in files if path.suffix.lower() in {".txt", ".md", ".json"}]
        values["inputs"] = await adapter.upload_import_inputs(
            [{"path": path, "format": "json" if path.suffix.lower() == ".json" else values.get("input_mode", "text")} for path in files],
            operation_id=values["operation_id"],
        )
    result = await adapter.call("memory_import_admin", {"action": action, **values})
    if action in {"settings", "get_settings", "get_guide"} and adapter.transfer_root is not None:
        result["settings"]["path_aliases"] = adapter.managed_files.path_aliases()
    return result
