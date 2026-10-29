"""Inspect actual resource configuration, calls, prompts and durable work."""

import asyncio
import re
import tempfile
from pathlib import Path
from typing import Literal
from uuid import uuid4

import anyio
from fastapi import APIRouter, HTTPException, Request
from fastapi.responses import FileResponse
from nous_wave.contracts.model_roles import ModelContractRequest, ModelPreviewRequest
from pydantic import BaseModel, Field, JsonValue

from nous.platform.instance import initialize
from nous.platform.persistence import OperationConflict

from .security import application, authorize, context

router = APIRouter(prefix="/api")


@router.post("/models/contract")
async def model_contract(
    request: Request,
    command: ModelContractRequest,
    namespace_id: str,
    subject_id: str | None = None,
):
    ctx = await context(request, namespace_id, subject_id, capability="read")
    return await application(request).models.model_contract(ctx, command)


@router.post("/models/preview")
async def model_preview(
    request: Request, command: ModelPreviewRequest, namespace_id: str, subject_id: str | None = None
):
    ctx = await context(request, namespace_id, subject_id, capability="read")
    return await application(request).models.model_preview(ctx, command)


@router.post("/backups")
async def backup(request: Request) -> dict[str, object]:
    (await authorize(request, capability="manage")).require_instance_management()
    app = application(request)
    target = app.settings.instance / "backups" / f"nous-{uuid4()}.zip"
    target.parent.mkdir(parents=True, exist_ok=True)
    result = await app.backups.create(target)
    return {
        "filename": target.name,
        "commit_seq": result.commit_seq,
        "object_count": result.object_count,
    }


@router.get("/backups/{filename}")
async def download_backup(request: Request, filename: str) -> FileResponse:
    (await authorize(request, capability="manage")).require_instance_management()
    if (
        not filename.startswith("nous-")
        or not filename.endswith(".zip")
        or "/" in filename
        or "\\" in filename
    ):
        raise HTTPException(status_code=404, detail="Backup does not exist")
    path = application(request).settings.instance / "backups" / filename
    if not path.is_file():
        raise HTTPException(status_code=404, detail="Backup does not exist")
    return FileResponse(path, media_type="application/zip", filename=filename)


@router.post("/backups/restore")
async def restore_backup(request: Request, instance_name: str) -> dict[str, object]:
    (await authorize(request, capability="manage")).require_instance_management()
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,63}", instance_name):
        raise ValueError("New instance name requires 1–64 letters, numbers, underscores or hyphens")
    app = application(request)
    target = app.settings.instance.resolve().parent / instance_name
    if target.exists():
        raise OperationConflict("The destination instance already exists")
    staging_root = app.settings.instance / "backups"
    staging_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="restore-upload-", dir=staging_root) as temporary:
        archive = Path(temporary) / "backup.zip"
        length = 0
        async with await anyio.open_file(archive, "xb") as output:
            async for chunk in request.stream():
                length += len(chunk)
                if length > 100 * 1024**3:
                    raise ValueError("Backup upload exceeds the 100 GiB restore limit")
                await output.write(chunk)
        try:
            restored = await app.backups.restore(archive, target)
        except FileExistsError as error:
            raise OperationConflict("The destination instance already exists") from error
    token_file = initialize(restored.path)
    return {
        "instance": str(restored.path),
        "commit_seq": restored.commit_seq,
        "object_count": restored.object_count,
        "credential_file": str(token_file),
        "status": "restored",
    }


class ConfigChange(BaseModel):
    value: JsonValue
    scope: Literal["instance", "namespace", "subject"] = "instance"
    operation_id: str = Field(default_factory=lambda: str(uuid4()))


class PromptChange(BaseModel):
    template: str
    operation_id: str = Field(default_factory=lambda: str(uuid4()))


class PromptActivation(BaseModel):
    revision: int = Field(ge=0)
    operation_id: str = Field(default_factory=lambda: str(uuid4()))


class PromptPreview(BaseModel):
    variables: dict[str, JsonValue]
    template: str | None = None


@router.get("/prompts")
async def prompts(request: Request, namespace_id: str = "instance"):
    return await application(request).prompts.list(
        await context(request, namespace_id, capability="read")
    )


@router.get("/prompts/{prompt_id}/history")
async def prompt_history(request: Request, prompt_id: str, namespace_id: str = "instance"):
    return await application(request).prompts.history(
        await context(request, namespace_id, capability="read"), prompt_id
    )


@router.post("/prompts/{prompt_id}/activate")
async def activate_prompt(
    request: Request, prompt_id: str, command: PromptActivation, namespace_id: str = "instance"
):
    (await authorize(request, capability="manage")).require_instance_management()
    return await application(request).prompts.activate(
        await context(request, namespace_id, capability="manage"),
        prompt_id,
        command.revision,
        command.operation_id,
    )


@router.post("/prompts/{prompt_id}/preview")
async def preview_prompt(
    request: Request, prompt_id: str, command: PromptPreview, namespace_id: str = "instance"
):
    ctx = await context(request, namespace_id, capability="read")
    service = application(request).prompts
    if command.template is not None:
        return service.preview(
            {"id": prompt_id, "revision": "draft", "template": command.template},
            dict(command.variables),
        )
    return await service.render(ctx, dict(command.variables), prompt_id)


@router.get("/status")
async def status(request: Request):
    await authorize(request)
    app = application(request)
    async with app.authority.read() as view:
        return {
            "status": "running",
            "commit_seq": view.commit_seq,
            "instance": str(app.settings.instance),
            "application": "nous-wave-server",
        }


@router.get("/configuration")
async def configuration_catalog(
    request: Request, namespace_id: str = "instance", subject_id: str | None = None
):
    await context(request, namespace_id, subject_id, capability="read")
    return application(request).configuration.catalog()


@router.get("/configuration/{key}/schema")
async def configuration_schema(
    request: Request, key: str, namespace_id: str = "instance", subject_id: str | None = None
):
    await context(request, namespace_id, subject_id, capability="read")
    return application(request).configuration.describe(key)


@router.get("/configuration/{key}")
async def configuration(
    request: Request, key: str, namespace_id: str = "instance", subject_id: str | None = None
):
    return await application(request).configuration.effective(
        await context(request, namespace_id, subject_id, capability="read"), key
    )


@router.get("/configuration/{key}/history")
async def configuration_history(
    request: Request,
    key: str,
    namespace_id: str = "instance",
    subject_id: str | None = None,
    limit: int = 100,
):
    return await application(request).configuration.history(
        await context(request, namespace_id, subject_id, capability="read"), key, limit
    )


@router.put("/configuration/{key}")
async def configure(
    request: Request,
    key: str,
    command: ConfigChange,
    namespace_id: str = "instance",
    subject_id: str | None = None,
):
    principal = await authorize(request, capability="manage")
    service = application(request).configuration
    if command.scope == "instance" or service.entry(key).instance_management:
        principal.require_instance_management()
    return await service.set(
        await context(request, namespace_id, subject_id, capability="manage"),
        key,
        command.value,
        command.operation_id,
        command.scope,
    )


@router.get("/prompts/{prompt_id}")
async def prompt(request: Request, prompt_id: str, namespace_id: str = "instance"):
    return await application(request).prompts.get(
        await context(request, namespace_id, capability="read"), prompt_id
    )


@router.put("/prompts/{prompt_id}")
async def save_prompt(
    request: Request, prompt_id: str, command: PromptChange, namespace_id: str = "instance"
):
    (await authorize(request, capability="manage")).require_instance_management()
    return await application(request).prompts.save(
        await context(request, namespace_id, capability="manage"),
        prompt_id,
        command.template,
        command.operation_id,
    )


@router.get("/model-calls")
async def model_calls(request: Request, namespace_id: str, subject_id: str | None = None):
    return await application(request).models.ledger.list(
        await context(request, namespace_id, subject_id, capability="read")
    )


@router.get("/jobs/{job_id}")
async def job(request: Request, job_id: str, namespace_id: str, subject_id: str | None = None):
    app = application(request)
    value = await app.execution.status(
        await context(request, namespace_id, subject_id, capability="read"), job_id
    )
    return {"job": value}


@router.post("/jobs/{job_id}/{action}")
async def job_action(
    request: Request,
    job_id: str,
    action: Literal["cancel", "resume", "wait"],
    namespace_id: str,
    subject_id: str | None = None,
    wait_seconds: float = 120,
):
    app = application(request)
    ctx = await context(
        request, namespace_id, subject_id, capability="read" if action == "wait" else "write"
    )
    if action == "wait":
        async with asyncio.timeout(wait_seconds):
            return await app.execution.wait(ctx, job_id)
    if action == "cancel":
        await app.execution.cancel(ctx, job_id)
    else:
        await app.execution.resume(ctx, job_id)
    return {"id": job_id, "action": action}
