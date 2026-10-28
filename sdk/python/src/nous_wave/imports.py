from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import TYPE_CHECKING, Any
from uuid import NAMESPACE_URL, uuid4, uuid5

from .files import ManagedFiles, file_name

if TYPE_CHECKING:
    from .client import NousClient


async def upload_import_inputs(
    client: "NousClient", namespace_id: str, subject_id: str,
    files: Sequence[str | Path | Mapping[str, Any]], *, transfer_root: str | Path,
    input_format: str | None = None,
    operation_id: str | None = None,
) -> list[dict[str, Any]]:
    managed = ManagedFiles(transfer_root)
    documents = []
    for value in files:
        item = dict(value) if isinstance(value, Mapping) else {"path": value}
        raw_path = item.get("staged_path") or item.get("path")
        if raw_path is None:
            raise ValueError("Import file requires staged_path or path")
        path = managed.local_file(raw_path)
        name = file_name(item.get("filename") or item.get("name") or path.name)
        format_ = item.get("format") or input_format or {
            ".txt": "text", ".md": "text", ".json": "json", ".jsonl": "jsonl",
        }.get(Path(name).suffix.lower())
        if format_ not in {"text", "json", "jsonl"}:
            raise ValueError("Import file format must be text, json or jsonl")
        source = item.get("source", name)
        if source is not None and not isinstance(source, str):
            raise ValueError("Import source must be text or None")
        documents.append((path, name, format_, source))
    if not documents:
        raise ValueError("Select at least one import file")
    inputs = []
    for index, (path, name, format_, source) in enumerate(documents):
        receipt = await client.upload_file(
            namespace_id, path, owner_subject_id=subject_id,
            media_type="application/octet-stream", source=name,
            operation_id=str(uuid5(NAMESPACE_URL, f"nous-wave:import-upload:{namespace_id}:{subject_id}:{operation_id}:{index}"))
                if operation_id is not None else None,
        )
        inputs.append({"name": name, "format": format_, "source": source,
                       "input": {"kind": "material", "material_id": receipt["result"]["id"]}})
    return inputs


async def import_files(
    client: "NousClient", namespace_id: str, subject_id: str,
    files: Sequence[str | Path | Mapping[str, Any]], *, transfer_root: str | Path,
    input_format: str | None = None, form_memory: bool = False, completion: str = "accepted",
    origin: Mapping[str, Any] | None = None, operation_id: str | None = None,
) -> dict:
    if completion not in {"accepted", "formed"}:
        raise ValueError("Import completion must be accepted or formed")
    if completion == "formed" and not form_memory:
        raise ValueError("formed import completion requires form_memory=True")
    operation_id = operation_id or str(uuid4())
    inputs = await upload_import_inputs(
        client, namespace_id, subject_id, files,
        transfer_root=transfer_root, input_format=input_format, operation_id=operation_id,
    )
    return await client.import_text({
        "namespace_id": namespace_id, "subject_id": subject_id, "inputs": inputs,
        "form_memory": form_memory, "completion": completion,
        "origin": dict(origin) if origin is not None else None,
    }, operation_id=operation_id)


async def import_directory(
    client: "NousClient", namespace_id: str, subject_id: str, *, transfer_root: str | Path,
    alias: str = "raw", relative_path: str = "", pattern: str = "*", recursive: bool = True,
    input_format: str | None = None, form_memory: bool = False, completion: str = "accepted",
    origin: Mapping[str, Any] | None = None, operation_id: str | None = None,
) -> dict:
    files = await ManagedFiles(transfer_root).scan(alias, relative_path, pattern=pattern, recursive=recursive)
    return await import_files(
        client, namespace_id, subject_id, files,
        transfer_root=transfer_root, input_format=input_format, form_memory=form_memory,
        completion=completion, origin=origin, operation_id=operation_id,
    )
