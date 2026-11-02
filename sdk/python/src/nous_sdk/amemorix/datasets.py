"""Legacy source paths are captured on the caller and processed by native dataset jobs."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import TYPE_CHECKING

import anyio
from pydantic import TypeAdapter

from ..files import file_name
from ..imports import upload_import_file_documents
from ..operations import encoded
from ..types import JsonObject

if TYPE_CHECKING:
    from .client import AMemorixAdapter

CREATE_DATASETS = frozenset(
    {
        "create_lpmm_openie",
        "create_lpmm_convert",
        "create_maibot_migration",
        "create_temporal_backfill",
    }
)


async def capture(adapter: AMemorixAdapter, action: str, arguments: JsonObject) -> JsonObject:
    values = dict(arguments)
    if values.get("inputs"):
        return values
    if action == "create_temporal_backfill" and not any(
        key in values for key in ("source_db", "alias", "relative_path")
    ):
        return values
    managed = adapter.managed_files
    relative = TypeAdapter(str).validate_python(values.pop("relative_path", ""))
    files: list[Path] = []
    graph_pickle: Path | None = None
    alias = (
        "lpmm"
        if action in {"create_lpmm_openie", "create_lpmm_convert"}
        else "maibot"
        if action == "create_maibot_migration"
        else "converted"
    )
    supplied_alias = values.pop("alias", alias)
    if supplied_alias != alias:
        raise ValueError(f"{action} requires the {alias} transfer alias")
    if action == "create_lpmm_openie":
        include_all = TypeAdapter(bool).validate_python(values.pop("include_all_json", False))
        files = await managed.scan(alias, relative, pattern="*-openie.json", recursive=True)
        if not files and include_all:
            files = await managed.scan(alias, relative, pattern="*.json", recursive=True)
    elif action == "create_lpmm_convert":
        target_alias = values.get("target_alias", "converted")
        if target_alias == "plugin_data":
            target_alias = "converted"
        if target_alias != "converted":
            raise ValueError("Converted artifacts use the converted transfer alias")
        target_relative = TypeAdapter(str).validate_python(values.get("target_relative_path", ""))
        destination = managed.resolve_alias("converted", target_relative, must_exist=False)
        if await anyio.Path(destination).exists():
            if not await anyio.Path(destination).is_dir() or await anyio.to_thread.run_sync(
                lambda: any(path.name != "dataset.nous.zip" for path in destination.iterdir())
            ):
                raise FileExistsError("Converted destination contains unrelated existing files")
        values.update(target_alias="converted", target_relative_path=target_relative)
        root = managed.resolve_alias(alias, relative)
        if not await anyio.Path(root).is_dir():
            raise ValueError("LPMM conversion requires its actual dataset directory")
        for name in ("entity.parquet", "paragraph.parquet", "relation.parquet"):
            candidate = root / name
            if await anyio.Path(candidate).is_file():
                files.append(managed.local_file(candidate))
        if not any(path.name in {"entity.parquet", "paragraph.parquet"} for path in files):
            raise ValueError("LPMM source requires paragraph.parquet or entity.parquet")
        graphs = [
            root / name
            for name in ("rag-graph.graphml", "graph.graphml")
            if await anyio.Path(root / name).is_file()
        ]
        if graphs:
            files.append(managed.local_file(graphs[0]))
        elif await anyio.Path(root / "graph_structure.pkl").exists():
            graph_pickle = managed.local_file(root / "graph_structure.pkl")
    else:
        source = values.pop("source_db", None)
        if source is not None:
            files = [managed.local_file(TypeAdapter(str).validate_python(source))]
        else:
            files = await managed.scan(alias, relative, pattern="*.db", recursive=True)
            files.extend(await managed.scan(alias, relative, pattern="*.sqlite", recursive=True))
            files.extend(await managed.scan(alias, relative, pattern="*.sqlite3", recursive=True))
            if len(files) != 1:
                raise ValueError("Select one actual source SQLite database")
    if not files:
        raise ValueError("No actual source files match the requested dataset")
    operation = TypeAdapter(str).validate_python(values["operation_id"])
    if graph_pickle is not None:
        graph_originals = await upload_import_file_documents(
            adapter.client,
            adapter.namespace_id,
            
            (graph_pickle,),
            transfer_root=managed.root,
            operation_id=operation + ":graph-original",
            input_format="media",
        )
        values["graph_pickle_material_id"] = graph_originals[0].material_id
    format_ = (
        "json"
        if action == "create_lpmm_openie"
        else "sqlite"
        if action in {"create_maibot_migration", "create_temporal_backfill"}
        else None
    )
    documents = await upload_import_file_documents(
        adapter.client,
        adapter.namespace_id,
        
        files,
        transfer_root=managed.root,
        operation_id=operation,
        input_format=format_,
    )
    origin = json.dumps([alias, relative], ensure_ascii=False, separators=(",", ":"))
    values["inputs"] = [
        encoded(
            document.model_copy(
                update={
                    "origin": {**document.origin, "dataset_ref": origin},
                    "source_type": "maibot_history"
                    if action == "create_maibot_migration"
                    else "legacy_metadata"
                    if action == "create_temporal_backfill"
                    else "lpmm_" + Path(document.filename).stem
                    if document.format == "parquet"
                    else "lpmm_openie"
                    if action == "create_lpmm_openie"
                    else "source_graph",
                }
            )
        )
        for document in documents
    ]
    return values


def _digest_file(path: Path) -> str:
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


async def receive(adapter: AMemorixAdapter, result: JsonObject) -> JsonObject:
    task = result.get("task")
    if (
        not isinstance(task, dict)
        or task.get("status") != "completed"
        or not task.get("artifact_material_id")
        or adapter.transfer_root is None
    ):
        return result
    delivery = task.get("delivery")
    if not isinstance(delivery, dict):
        return result
    alias = TypeAdapter(str).validate_python(delivery.get("alias"))
    relative = TypeAdapter(str).validate_python(delivery.get("relative_path", ""))
    if alias != "converted":
        raise ValueError("Dataset artifacts use the converted transfer alias")
    directory = adapter.managed_files.resolve_alias(alias, relative, must_exist=False)
    await anyio.Path(directory).mkdir(parents=True, exist_ok=True)
    target = adapter.managed_files.contained(directory / file_name(task.get("artifact_filename")))
    if await anyio.Path(target).exists():
        if await anyio.to_thread.run_sync(_digest_file, target) != task.get(
            "artifact_content_hash"
        ):
            raise FileExistsError("Converted output exists and contains different bytes")
    else:
        await adapter.client.download_material(
            TypeAdapter(str).validate_python(task["artifact_material_id"]),
            target,
            namespace_id=adapter.namespace_id,
            
        )
    task = {**task, "artifact_paths": {"output_archive": str(target)}}
    return TypeAdapter(JsonObject).validate_python({**result, "task": task})
