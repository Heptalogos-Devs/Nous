"""Import administration presents catalogs, immutable outputs and real durable jobs."""

from typing import TYPE_CHECKING, Literal, get_args
from uuid import uuid4

from nous_sdk.contracts import (
    AdmitFiles,
    CallContext,
    Contract,
    DomainError,
    FreezeCatalog,
    ImportDocument,
    ProcessCatalog,
    TimePoint,
    TimeRange,
    UnitOutput,
)
from nous_sdk.contracts.dataset_imports import BackfillSourceTimes, ImportDataset
from nous_sdk.contracts.imports import (
    FormationOptionsOverride,
    ImportFormat,
    RecordSelection,
    TextSplitOptions,
)
from nous_sdk.contracts.namespace_policy import FormationPolicy
from pydantic import Field, JsonValue, TypeAdapter
from sqlalchemy import select

from nous.imports.schema import inputs, outputs
from nous.platform.persistence.schema import job_links, operations
from nous.platform.time import parse_source_time

from .helpers import json_object

if TYPE_CHECKING:
    from .service import AMemorixService


class ImportInput(Contract):
    action: str
    name: str = "notes.txt"
    content: str | None = None
    inputs: tuple[ImportDocument, ...] = ()
    graph_pickle_material_id: str | None = None
    task_id: str | None = None
    catalog_id: str | None = None
    parent_catalog_id: str | None = None
    operation_id: str = Field(default_factory=lambda: str(uuid4()))
    llm_enabled: bool = True
    vectors: Literal["disabled", "metadata_allowed", "required"] = "metadata_allowed"
    dedupe_policy: Literal["none", "content_hash", "manifest"] = "none"
    force: bool = False
    clear_manifest: bool = False
    chunk_size: int = Field(default=2400, ge=128, le=100000)
    chunk_overlap: int = Field(default=200, ge=0)
    include_chunks: bool = False
    limit: int = Field(default=50, ge=1, le=10000000)
    offset: int = Field(default=0, ge=0)
    chat_id: str = ""
    scope_type: Literal["global", "chat"] | None = None
    input_mode: ImportFormat = "text"
    file_id: str | None = None
    overrides: dict[str, JsonValue] | None = None
    strategy_override: Literal["auto", "factual", "narrative", "quote", "chat_log"] | None = "auto"
    chat_log: bool = False
    chat_reference_time: str | None = None
    narrative_window_size: int | None = Field(default=None, ge=200)
    narrative_overlap: int | None = Field(default=None, ge=0)
    factual_target_size: int | None = Field(default=None, ge=200)
    extract_time: bool = True
    content_category: str | None = None
    timezone: str | None = None
    reference_time: TimePoint | None = None
    dry_run: bool = False
    no_created_fallback: bool = False
    dimension: int | None = Field(default=None, gt=0)
    batch_size: int = Field(default=1024, ge=1, le=1000000)
    read_batch_size: int | None = Field(default=None, ge=1, le=1000000)
    max_errors: int = Field(default=0, ge=0)
    preview_limit: int = Field(default=20, ge=1, le=1000)
    start_id: int | None = Field(default=None, ge=1)
    end_id: int | None = Field(default=None, ge=1)
    time_from: str | float | None = None
    time_to: str | float | None = None
    stream_ids: tuple[str, ...] = ()
    group_ids: tuple[str, ...] = ()
    user_ids: tuple[str, ...] = ()
    target_alias: str = "converted"
    target_relative_path: str = ""


class QueuedImport(Contract):
    request_id: str
    job_id: str
    catalog_id: str | None = None
    skipped_success_ids: tuple[str, ...] = ()


async def _submission(
    service: AMemorixService, ctx: CallContext, job_id: str
) -> dict[str, JsonValue]:
    await service.app.execution.status(ctx, job_id)
    async with service.app.authority.read(ctx) as view:
        raw = (
            await view.connection.execute(
                select(operations.c.result_json)
                .join(job_links, job_links.c.operation_key == operations.c.operation_key)
                .where(
                    job_links.c.workflow_id == job_id,
                    job_links.c.namespace_id == ctx.namespace_id,
                    
                )
            )
        ).scalar_one_or_none()
    if raw is None:
        raise DomainError("task_not_found", "Import task has no authoritative submission")
    return TypeAdapter(dict[str, JsonValue]).validate_json(raw)


async def _detail(
    service: AMemorixService,
    ctx: CallContext,
    job_id: str,
    include_chunks: bool,
    offset: int,
    limit: int,
    file_id: str | None = None,
) -> dict[str, JsonValue]:
    submission = await _submission(service, ctx, job_id)
    status = await service.app.execution.status(ctx, job_id)
    catalog_id = submission.get("catalog_id")
    if not isinstance(catalog_id, str):
        if status.status == "SUCCESS":
            result = await service.app.execution.wait(ctx, job_id)
            if isinstance(result, dict) and isinstance(catalog := result.get("catalog"), dict):
                catalog_id = catalog.get("id")
    task: dict[str, JsonValue] = {
        "task_id": job_id,
        "job_id": job_id,
        "catalog_id": catalog_id,
        "status": {
            "SUCCESS": "completed",
            "ERROR": "failed",
            "CANCELLED": "cancelled",
            "MAX_RECOVERY_ATTEMPTS_EXCEEDED": "failed",
            "PENDING": "running",
            "ENQUEUED": "queued",
            "DELAYED": "queued",
        }[status.status],
        "created_at": status.created_at,
        "updated_at": status.updated_at,
        "error": str(status.error) if status.error else None,
    }
    if isinstance(submission.get("delivery"), dict):
        task["delivery"] = submission["delivery"]
    if status.status == "SUCCESS":
        completed = await service.app.execution.wait(ctx, job_id)
        if isinstance(completed, dict):
            for key in (
                "artifact_material_id",
                "artifact_filename",
                "artifact_content_hash",
                "artifact_bytes",
                "artifact_format",
                "errors",
                "dry_run",
                "candidates",
                "updated",
                "sample_unit_ids",
                "scanned",
                "references",
            ):
                if key in completed:
                    task[key] = completed[key]
    chunks: list[dict[str, JsonValue]] = []
    files: list[dict[str, JsonValue]] = []
    if isinstance(catalog_id, str):
        catalog = await service.app.imports.read_catalog(ctx, catalog_id)
        attempts = await service.app.imports.read_attempts(ctx, catalog_id)
        latest = {
            attempt.unit_id: attempt
            for attempt in sorted(attempts, key=lambda item: item.started_at)
        }
        async with service.app.authority.read(ctx) as view:
            adopted = [
                UnitOutput.model_validate(value)
                for value in (
                    await view.connection.execute(
                        select(outputs.c.record).where(
                            outputs.c.unit_id.in_([unit.id for unit in catalog.units])
                        )
                    )
                ).scalars()
            ]
            originals = list(
                (
                    await view.connection.execute(
                        select(inputs.c.record).where(inputs.c.catalog_id == catalog_id)
                    )
                ).scalars()
            )
        by_unit = {output.unit_id: output for output in adopted}
        all_chunks: list[dict[str, JsonValue]] = []
        for unit in catalog.units:
            output = by_unit.get(unit.id)
            attempt = latest.get(unit.id)
            state = "completed" if output else attempt.status if attempt is not None else "pending"
            all_chunks.append(
                json_object(
                    {
                        "chunk_id": unit.id,
                        "unit_id": unit.id,
                        "chunk_type": unit.strategy,
                        "file_id": unit.input_source_id,
                        "index": unit.ordinal,
                        "content_preview": unit.text[:120],
                        "status": state,
                        "step": attempt.stage if attempt else None,
                        "retryable": attempt is not None
                        and attempt.status in {"failed", "cancelled"},
                        "error": attempt.error if attempt else None,
                        "vector_error": attempt.vector_error if attempt else None,
                        "native_job_id": attempt.job_id if attempt else None,
                        "output": output,
                        "updated_at": attempt.finished_at or attempt.started_at
                        if attempt
                        else None,
                    }
                )
            )
        for original in originals:
            rows = [item for item in all_chunks if item["file_id"] == original["id"]]
            done = sum(item["status"] == "completed" for item in rows)
            failed = sum(item["status"] == "failed" for item in rows)
            cancelled = sum(item["status"] == "cancelled" for item in rows)
            document = ImportDocument.model_validate(original["document"])
            file_state = (
                "completed"
                if rows and done == len(rows)
                else "failed"
                if rows and failed == len(rows)
                else "completed_with_errors"
                if rows and done + failed + cancelled == len(rows)
                else "running"
                if any(item["status"] == "running" for item in rows)
                else "pending"
            )
            files.append(
                json_object(
                    {
                        "file_id": original["id"],
                        "name": document.filename,
                        "input_mode": document.format,
                        "status": file_state,
                        "current_step": file_state,
                        "total_chunks": len(rows),
                        "done_chunks": done,
                        "failed_chunks": failed,
                        "cancelled_chunks": cancelled,
                        "progress": done / len(rows) if rows else None,
                        "content_hash": original["content_hash"],
                        "source_path": None,
                        "chunks": rows if include_chunks else [],
                    }
                )
            )
        filtered = [item for item in all_chunks if file_id is None or item["file_id"] == file_id]
        chunks = filtered[offset : offset + limit] if include_chunks else []
        task.update(
            json_object(
                {
                    "total_chunks": len(all_chunks),
                    "done_chunks": sum(item["status"] == "completed" for item in all_chunks),
                    "failed_chunks": sum(item["status"] == "failed" for item in all_chunks),
                    "cancelled_chunks": sum(item["status"] == "cancelled" for item in all_chunks),
                    "file_count": len(files),
                    "record_errors": json_object({"errors": catalog.errors})["errors"],
                    "files": files,
                }
            )
        )
    return json_object({"success": True, "task": task, "chunks": chunks, "files": files})


async def import_admin(
    service: AMemorixService, ctx: CallContext, arguments: dict[str, JsonValue]
) -> dict[str, JsonValue]:
    command = ImportInput.model_validate(arguments)
    retry_values = {**arguments, **(command.overrides or {})}
    formation_override = (
        FormationOptionsOverride.model_validate(
            {
                key: retry_values[key]
                for key in FormationOptionsOverride.model_fields
                if key in retry_values
            }
        )
        if command.action == "retry_failed"
        else None
    )
    if command.action == "retry_failed" and command.overrides is not None:
        command = ImportInput.model_validate(
            {**command.model_dump(), **command.overrides, "overrides": None}
        )
    if (command.scope_type == "chat" and not command.chat_id) or (
        command.scope_type == "global" and command.chat_id
    ):
        raise DomainError("import_scope_invalid", "Chat import scope requires its actual chat ID")
    if command.action in {"settings", "get_settings", "get_guide"}:
        policy = FormationPolicy.model_validate(
            (await service.app.configuration.effective(ctx, "formation"))["value"]
        )
        split = TextSplitOptions()
        return json_object(
            {
                "success": True,
                "settings": {
                    "enabled": True,
                    "formats": list(get_args(ImportFormat)),
                    "chunk_size": min(split.chunk_size, policy.max_chunk_chars),
                    "chunk_overlap": policy.chunk_overlap_chars,
                    "dedupe_policies": ["none", "content_hash", "manifest"],
                    "llm_enabled": policy.default_enabled,
                    "formation_enabled": policy.enabled,
                    "capabilities": {
                        "file_dedupe": True,
                        "json_processing": True,
                        "unit_retry": True,
                        "request_concurrency": False,
                    },
                    "shared_task_concurrency": next(
                        queue.worker_concurrency
                        for queue in service.app.execution.queues
                        if queue.name == "formation"
                    ),
                    "default_narrative_window_size": min(split.chunk_size, policy.max_chunk_chars),
                    "default_narrative_overlap": policy.chunk_overlap_chars,
                    "default_factual_target_size": min(split.chunk_size, policy.max_chunk_chars),
                },
                "guide": "Upload originals, freeze a catalog, and process its stable units. Failed units retry in the same catalog.",
            }
        )
    if command.action in {
        "create_lpmm_openie",
        "create_lpmm_convert",
        "create_maibot_migration",
        "create_temporal_backfill",
    }:
        if command.action == "create_temporal_backfill" and not command.inputs:
            queued = await service.app.datasets.submit_time_backfill(
                ctx,
                BackfillSourceTimes(
                    operation_id=command.operation_id,
                    limit=command.limit if "limit" in arguments else 100000,
                    dry_run=command.dry_run,
                    no_created_fallback=command.no_created_fallback,
                ),
            )
            return await _detail(
                service, ctx, str(queued["job_id"]), False, 0, min(command.limit, 1000)
            )
        if not command.inputs:
            raise DomainError(
                "inputs_required", "Dataset conversion requires captured source originals"
            )
        if command.graph_pickle_material_id is not None:
            if command.action != "create_lpmm_convert":
                raise DomainError(
                    "lpmm_graph_action", "Legacy graph archives belong to LPMM conversion"
                )
            graph_material = await service.app.datasets.convert_lpmm_graph(
                ctx, command.graph_pickle_material_id
            )
            command = command.model_copy(
                update={
                    "inputs": (
                        *command.inputs,
                        ImportDocument(
                            material_id=graph_material,
                            format="graphml",
                            filename="graph.graphml",
                            media_type="application/graphml+xml",
                            source_type="source_graph",
                            origin=command.inputs[0].origin,
                        ),
                    )
                }
            )
        begin, end = parse_source_time(command.time_from), parse_source_time(command.time_to)
        if any(
            raw not in (None, "") and parsed is None
            for raw, parsed in ((command.time_from, begin), (command.time_to, end))
        ):
            raise DomainError(
                "time_scope_invalid",
                "Migration bounds require actual dates or numeric source timestamps",
            )
        if begin and end and begin.domain != end.domain:
            raise DomainError(
                "time_scope_invalid", "Migration bounds must share the same actual time domain"
            )
        selection = RecordSelection(
            after_sequence=command.start_id - 1 if command.start_id is not None else None,
            through_sequence=command.end_id,
            conversation_refs=command.stream_ids or None,
            group_refs=command.group_ids,
            user_refs=command.user_ids,
            event=TimeRange(
                domain=begin.domain if begin else end.domain if end else "unix_ms",
                start=begin.coordinate if begin else None,
                end=end.coordinate if end else None,
            )
            if begin or end
            else None,
            limit=command.limit
            if "limit" in arguments
            else 100000
            if command.action == "create_temporal_backfill"
            else None,
        )
        documents = tuple(
            document.model_copy(
                update={
                    "origin": {
                        **document.origin,
                        **({"conversation_ref": command.chat_id} if command.chat_id else {}),
                        "dataset_ref": document.origin.get("dataset_ref")
                        or "operation:" + command.operation_id,
                        **({"no_created_fallback": "true"} if command.no_created_fallback else {}),
                    },
                    "selection": document.selection or selection,
                    "declared_vector_dimensions": command.dimension
                    or document.declared_vector_dimensions,
                    "read_batch_size": command.read_batch_size or command.batch_size,
                }
            )
            for document in command.inputs
        )
        dataset = ImportDataset(
            operation_id=command.operation_id,
            inputs=documents,
            name=command.name,
            llm_enabled=command.llm_enabled if "llm_enabled" in arguments else False,
            vectors=command.vectors,
            export_bundle=command.action == "create_lpmm_convert",
            dry_run=command.dry_run,
            max_errors=command.max_errors,
            preview_limit=command.preview_limit,
        )
        catalog = await service.app.datasets.prepare(ctx, dataset)
        async with service.app.authority.write(
            ctx,
            command.operation_id,
            operation_kind="amemorix.dataset",
            scope=ctx.namespace_id,
            payload=command,
        ) as tx:
            if tx.replayed:
                queued = TypeAdapter(dict[str, JsonValue]).validate_python(tx.replay_result)
            else:
                queued = await service.app.datasets.enqueue_in(tx, ctx, dataset, catalog.id)
                if command.action == "create_lpmm_convert":
                    alias = (
                        "converted"
                        if command.target_alias == "plugin_data"
                        else command.target_alias
                    )
                    if alias != "converted":
                        raise DomainError(
                            "import_destination_invalid",
                            "Converted artifacts use the converted transfer alias",
                        )
                    queued["delivery"] = {
                        "alias": alias,
                        "relative_path": command.target_relative_path,
                    }
                tx.result(queued)
        return await _detail(
            service, ctx, str(queued["job_id"]), False, 0, min(command.limit, 1000)
        )
    if command.action == "list":
        jobs = [
            job
            for job in await service.app.execution.list_jobs(ctx, limit=min(command.limit, 1000))
            if job.name.startswith("nous.imports.")
        ]
        details = [
            await _detail(service, ctx, job.workflow_id, False, 0, command.limit) for job in jobs
        ]
        return {"success": True, "items": [detail["task"] for detail in details]}
    if command.action in {"get", "chunks", "get_chunks", "cancel", "retry_failed"}:
        if not command.task_id:
            raise DomainError("task_required", "Select an import task")
        if command.action == "cancel":
            await service.app.execution.cancel(ctx, command.task_id)
            return await _detail(service, ctx, command.task_id, False, 0, command.limit)
        if command.action == "retry_failed":
            submission = await _submission(service, ctx, command.task_id)
            catalog_id = submission.get("catalog_id")
            if not isinstance(catalog_id, str):
                raise DomainError("catalog_required", "The original task has no frozen catalog")
            catalog = await service.app.imports.read_catalog(ctx, catalog_id)
            split_fields = {
                "chunk_size",
                "chunk_overlap",
                "strategy_override",
                "chat_log",
                "narrative_window_size",
                "narrative_overlap",
                "factual_target_size",
            }
            explicit_split = split_fields.intersection(retry_values)
            if explicit_split and (
                catalog.split_options is None
                or any(
                    retry_values[key] != catalog.split_options.model_dump(mode="json")[key]
                    for key in explicit_split
                )
            ):
                raise DomainError(
                    "import_resplit_required",
                    "Changed splitting requires a new catalog with parent_catalog_id",
                )
            async with service.app.authority.read(ctx) as view:
                completed = {
                    str(value)
                    for value in (
                        await view.connection.execute(
                            select(outputs.c.unit_id).where(
                                outputs.c.unit_id.in_([unit.id for unit in catalog.units])
                            )
                        )
                    ).scalars()
                }
            attempts = await service.app.imports.read_attempts(ctx, catalog_id)
            latest = {
                attempt.unit_id: attempt
                for attempt in sorted(attempts, key=lambda item: item.started_at)
            }
            failed_ids = {
                attempt.unit_id
                for attempt in latest.values()
                if attempt.status in {"failed", "cancelled"}
            }
            failed = tuple(
                unit.id
                for unit in catalog.units
                if unit.id not in completed and unit.id in failed_ids
            )
            if not failed:
                raise DomainError("no_failed_units", "The frozen catalog has no failed units")
            receipt = await service.app.imports.submit_process(
                ctx,
                ProcessCatalog(
                    operation_id=command.operation_id,
                    catalog_id=catalog_id,
                    unit_ids=failed,
                    formation_options_override=formation_override,
                    llm_enabled=command.llm_enabled,
                    vectors=command.vectors,
                ),
            )
            return await _detail(
                service, ctx, QueuedImport.model_validate(receipt).job_id, False, 0, command.limit
            )
        return await _detail(
            service,
            ctx,
            command.task_id,
            command.include_chunks or command.action in {"chunks", "get_chunks"},
            command.offset,
            command.limit,
            command.file_id,
        )
    if command.action in {"create_upload", "create_raw_scan", "create_paste"}:
        documents = command.inputs
        if command.action == "create_paste":
            if not command.content:
                raise DomainError("content_required", "Paste import requires content")
            documents = (
                ImportDocument(
                    text=command.content,
                    filename=command.name,
                    format=command.input_mode,
                    origin={"conversation_ref": command.chat_id} if command.chat_id else {},
                ),
            )
        if not documents:
            raise DomainError("inputs_required", "Import requires uploaded originals")
        documents = tuple(
            document.model_copy(
                update={
                    "origin": {
                        **document.origin,
                        **({"conversation_ref": command.chat_id} if command.chat_id else {}),
                    }
                }
            )
            for document in documents
        )
        freeze = FreezeCatalog(
            operation_id=command.operation_id + ":freeze",
            inputs=documents,
            parent_catalog_id=command.parent_catalog_id,
            chunk_size=command.chunk_size,
            chunk_overlap=command.chunk_overlap,
            strategy_override=command.strategy_override,
            narrative_window_size=command.narrative_window_size,
            narrative_overlap=command.narrative_overlap,
            factual_target_size=command.factual_target_size,
            chat_log=command.chat_log,
            chat_reference_time=command.chat_reference_time,
            extract_time=command.extract_time,
            content_category=command.content_category,
            timezone=command.timezone,
            reference_time=command.reference_time,
        )
        if command.dedupe_policy != "none" or command.force or command.clear_manifest:
            admit = AdmitFiles.model_validate(
                {
                    **freeze.model_dump(),
                    "operation_id": command.operation_id,
                    "llm_enabled": command.llm_enabled,
                    "vectors": command.vectors,
                    "dedupe_policy": command.dedupe_policy,
                    "force": command.force,
                    "clear_manifest": command.clear_manifest,
                }
            )
            receipt = await service.app.imports.submit_admit_files(ctx, admit)
        else:
            catalog = await service.app.imports.freeze_catalog(ctx, freeze)
            receipt = await service.app.imports.submit_process(
                ctx,
                ProcessCatalog(
                    operation_id=command.operation_id,
                    catalog_id=catalog.id,
                    llm_enabled=command.llm_enabled,
                    vectors=command.vectors,
                ),
            )
        return await _detail(
            service, ctx, QueuedImport.model_validate(receipt).job_id, False, 0, command.limit
        )
    raise DomainError("action_invalid", "Unknown import administration action")
