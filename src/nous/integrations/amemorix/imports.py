"""Import administration presents catalogs, immutable outputs and real durable jobs."""

from typing import TYPE_CHECKING, Literal
from uuid import uuid4

from nous_wave.contracts import (
    AdmitFiles,
    CallContext,
    Contract,
    DomainError,
    FreezeCatalog,
    ImportDocument,
    ProcessCatalog,
    UnitOutput,
)
from pydantic import Field, JsonValue, TypeAdapter
from sqlalchemy import select

from nous.imports.service import outputs
from nous.platform.persistence.schema import job_links, operations

from .helpers import json_object

if TYPE_CHECKING:
    from .service import AMemorixService


class ImportInput(Contract):
    action: str
    name: str = "notes.txt"
    content: str | None = None
    inputs: tuple[ImportDocument, ...] = ()
    task_id: str | None = None
    catalog_id: str | None = None
    operation_id: str = Field(default_factory=lambda: str(uuid4()))
    llm_enabled: bool = True
    vectors: Literal["disabled", "metadata_allowed", "required"] = "metadata_allowed"
    dedupe_policy: Literal["none", "content_hash", "manifest"] = "none"
    force: bool = False
    clear_manifest: bool = False
    chunk_size: int = Field(default=2400, ge=128, le=100000)
    chunk_overlap: int = Field(default=200, ge=0)
    include_chunks: bool = False
    limit: int = Field(default=50, ge=1, le=1000)
    offset: int = Field(default=0, ge=0)
    chat_id: str = ""


class QueuedImport(Contract):
    request_id: str
    job_id: str
    catalog_id: str | None = None


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
                    job_links.c.subject_id == ctx.subject_id,
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
    chunks: list[dict[str, JsonValue]] = []
    if include_chunks and isinstance(catalog_id, str):
        catalog = await service.app.imports.read_catalog(ctx, catalog_id)
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
        by_unit = {output.unit_id: output for output in adopted}
        for unit in catalog.units[offset : offset + limit]:
            output = by_unit.get(unit.id)
            chunks.append(
                json_object(
                    {
                        "unit_id": unit.id,
                        "index": unit.ordinal,
                        "content": unit.text,
                        "status": "completed"
                        if output
                        else "failed"
                        if status.status in {"ERROR", "MAX_RECOVERY_ATTEMPTS_EXCEEDED"}
                        else "pending",
                        "output": output,
                        "error": str(status.error) if output is None and status.error else None,
                    }
                )
            )
    return json_object({"success": True, "task": task, "chunks": chunks})


async def import_admin(
    service: AMemorixService, ctx: CallContext, arguments: dict[str, JsonValue]
) -> dict[str, JsonValue]:
    command = ImportInput.model_validate(arguments)
    if command.action in {"settings", "get_settings", "get_guide"}:
        return json_object(
            {
                "success": True,
                "settings": {
                    "enabled": True,
                    "formats": ["text", "json", "jsonl"],
                    "chunk_size": 2400,
                    "chunk_overlap": 200,
                    "dedupe_policies": ["none", "content_hash", "manifest"],
                    "llm_enabled": True,
                },
                "guide": "Upload originals, freeze a catalog, and process its stable units. Failed units retry in the same catalog.",
            }
        )
    if command.action == "list":
        jobs = [
            job
            for job in await service.app.execution.list_jobs(ctx, limit=command.limit)
            if job.name.startswith("nous.imports.")
        ]
        details = [
            await _detail(service, ctx, job.workflow_id, False, 0, command.limit) for job in jobs
        ]
        return {"success": True, "tasks": [detail["task"] for detail in details]}
    if command.action in {"get", "chunks", "cancel", "retry_failed"}:
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
            failed = tuple(unit.id for unit in catalog.units if unit.id not in completed)
            if not failed:
                raise DomainError("no_failed_units", "The frozen catalog has no failed units")
            receipt = await service.app.imports.submit_process(
                ctx,
                ProcessCatalog(
                    operation_id=command.operation_id,
                    catalog_id=catalog_id,
                    unit_ids=failed,
                    llm_enabled=command.llm_enabled,
                    vectors=command.vectors,
                ),
            )
            return {
                "success": True,
                "task": await _detail(
                    service,
                    ctx,
                    QueuedImport.model_validate(receipt).job_id,
                    False,
                    0,
                    command.limit,
                ),
            }
        return await _detail(
            service,
            ctx,
            command.task_id,
            command.include_chunks or command.action == "chunks",
            command.offset,
            command.limit,
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
                    origin={"conversation_ref": command.chat_id} if command.chat_id else {},
                ),
            )
        if not documents:
            raise DomainError("inputs_required", "Import requires uploaded originals")
        if command.dedupe_policy != "none" or command.force or command.clear_manifest:
            admit = AdmitFiles(
                operation_id=command.operation_id,
                inputs=documents,
                chunk_size=command.chunk_size,
                chunk_overlap=command.chunk_overlap,
                llm_enabled=command.llm_enabled,
                vectors=command.vectors,
                dedupe_policy=command.dedupe_policy,
                force=command.force,
                clear_manifest=command.clear_manifest,
            )
            receipt = await service.app.imports.submit_admit_files(ctx, admit)
        else:
            catalog = await service.app.imports.freeze_catalog(
                ctx,
                FreezeCatalog(
                    operation_id=command.operation_id + ":freeze",
                    inputs=documents,
                    chunk_size=command.chunk_size,
                    chunk_overlap=command.chunk_overlap,
                ),
            )
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
