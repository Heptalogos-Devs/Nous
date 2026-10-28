"""Inspect actual resource configuration, calls, prompts and durable work."""

import asyncio
from typing import Literal
from uuid import uuid4

from fastapi import APIRouter, HTTPException, Request
from fastapi.responses import FileResponse
from pydantic import BaseModel, Field, JsonValue

from .security import application, authorize, context

router = APIRouter(prefix="/api")


@router.post("/backups")
async def backup(request: Request) -> dict[str, object]:
    await authorize(request, capability="manage")
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
    await authorize(request, capability="manage")
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


class ConfigChange(BaseModel):
    value: JsonValue
    scope: Literal["instance", "namespace", "subject"] = "instance"
    operation_id: str = Field(default_factory=lambda: str(uuid4()))


class PromptChange(BaseModel):
    template: str
    operation_id: str = Field(default_factory=lambda: str(uuid4()))


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


@router.get("/configuration/{key}")
async def configuration(
    request: Request, key: str, namespace_id: str = "instance", subject_id: str | None = None
):
    return await application(request).configuration.effective(
        await context(request, namespace_id, subject_id), key
    )


@router.put("/configuration/{key}")
async def configure(
    request: Request,
    key: str,
    command: ConfigChange,
    namespace_id: str = "instance",
    subject_id: str | None = None,
):
    return await application(request).configuration.set(
        await context(request, namespace_id, subject_id, capability="manage"),
        key,
        command.value,
        command.operation_id,
        command.scope,
    )


@router.get("/prompts/{prompt_id}")
async def prompt(request: Request, prompt_id: str, namespace_id: str = "instance"):
    return await application(request).prompts.get(await context(request, namespace_id), prompt_id)


@router.put("/prompts/{prompt_id}")
async def save_prompt(
    request: Request, prompt_id: str, command: PromptChange, namespace_id: str = "instance"
):
    return await application(request).prompts.save(
        await context(request, namespace_id, capability="manage"),
        prompt_id,
        command.template,
        command.operation_id,
    )


@router.get("/model-calls")
async def model_calls(request: Request, namespace_id: str, subject_id: str | None = None):
    return await application(request).models.ledger.list(
        await context(request, namespace_id, subject_id)
    )


@router.get("/jobs/{job_id}")
async def job(request: Request, job_id: str, namespace_id: str, subject_id: str | None = None):
    app = application(request)
    value = await app.execution.status(await context(request, namespace_id, subject_id), job_id)
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
