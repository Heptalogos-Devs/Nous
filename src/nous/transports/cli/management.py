"""Resource configuration, durable jobs and API key management."""

import os
from pathlib import Path
from uuid import uuid4

import typer
from nous_sdk import NousClient
from nous_sdk.contracts.credentials import CreateCredential, RotateCredential
from nous_sdk.contracts.model_roles import ModelContractRequest, ModelPreviewRequest
from nous_sdk.operations import encoded, segment
from pydantic import JsonValue

from .common import download, invoke, read_command, read_json


def _issue(ctx: typer.Context, path: str, payload: JsonValue, token_out: Path | None) -> None:
    saved = False
    target = None
    if token_out is not None:
        token_out = token_out.resolve()
        descriptor = os.open(token_out, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        target = os.fdopen(descriptor, "w", encoding="utf-8")

    async def run(api: NousClient) -> object:
        nonlocal saved
        result = await api.request("POST", path, body=payload)
        if (
            token_out is not None
            and target is not None
            and isinstance(result, dict)
            and isinstance(result.get("token"), str)
        ):
            target.write(str(result["token"]))
            target.flush()
            os.fsync(target.fileno())
            saved = True
            return {"record": result["record"], "token_file": str(token_out)}
        return result

    try:
        invoke(ctx, run)
    finally:
        if target is not None:
            target.close()
            if not saved and token_out is not None:
                token_out.unlink(missing_ok=True)


def register(cli: typer.Typer) -> None:
    @cli.command("model-contract")
    def model_contract(
        ctx: typer.Context,
        role: str,
        namespace_id: str,
        subject_id: str | None = None,
        variant: str | None = None,
    ) -> None:
        """Read the role's real consumer schemas and active Prompt without calling a model."""
        command = ModelContractRequest(role=role, variant=variant)
        invoke(
            ctx,
            lambda api: api.model_contract(
                {
                    **command.model_dump(mode="json"),
                    "namespace_id": namespace_id,
                    "subject_id": subject_id,
                }
            ),
        )

    @cli.command("model-preview")
    def model_preview(
        ctx: typer.Context, request: Path, namespace_id: str, subject_id: str | None = None
    ) -> None:
        """Read ModelPreviewRequest JSON and build the real invocation without a provider call."""
        command = read_command(request, ModelPreviewRequest)
        invoke(
            ctx,
            lambda api: api.model_preview(
                {
                    **command.model_dump(mode="json"),
                    "namespace_id": namespace_id,
                    "subject_id": subject_id,
                }
            ),
        )

    jobs = typer.Typer(help="Inspect and control durable work.", no_args_is_help=False)
    config = typer.Typer(
        help="Effective configuration and scoped overrides.", no_args_is_help=False
    )
    prompts = typer.Typer(
        help="Global prompt revisions; changes require instance management.", no_args_is_help=False
    )
    credentials = typer.Typer(
        help="Create, list and revoke scoped API keys.", no_args_is_help=False
    )
    for name, group in (
        ("jobs", jobs),
        ("config", config),
        ("prompts", prompts),
        ("credentials", credentials),
    ):
        cli.add_typer(group, name=name)

    @cli.command("status")
    def status(ctx: typer.Context) -> None:
        invoke(ctx, lambda api: api.request("GET", "/api/status"))

    @jobs.command("list")
    def list_jobs(
        ctx: typer.Context,
        namespace_id: str,
        subject_id: str | None = None,
        limit: int = 50,
        kind: str | None = None,
        state: str | None = None,
        scope: str | None = None,
    ) -> None:
        invoke(
            ctx,
            lambda api: api.jobs(
                namespace_id=namespace_id,
                subject_id=subject_id,
                limit=limit,
                kind=kind,
                state=state,
                scope=scope,
            ),
        )

    @jobs.command("page")
    def job_page(
        ctx: typer.Context,
        namespace_id: str,
        subject_id: str | None = None,
        limit: int = 50,
        kind: str | None = None,
        state: str | None = None,
        scope: str | None = None,
        cursor: str | None = None,
    ) -> None:
        """Page durable jobs with exact kind/state filters; reuse next_cursor."""
        invoke(
            ctx,
            lambda api: api.job_page(
                namespace_id=namespace_id,
                subject_id=subject_id,
                limit=limit,
                kind=kind,
                state=state,
                scope=scope,
                cursor=cursor,
            ),
        )

    @jobs.command("status")
    def job(
        ctx: typer.Context, namespace_id: str, job_id: str, subject_id: str | None = None
    ) -> None:
        invoke(ctx, lambda api: api.job(job_id, namespace_id=namespace_id, subject_id=subject_id))

    @jobs.command("cancel")
    def cancel(
        ctx: typer.Context, namespace_id: str, job_id: str, subject_id: str | None = None
    ) -> None:
        invoke(
            ctx,
            lambda api: api.cancel_job(job_id, namespace_id=namespace_id, subject_id=subject_id),
        )

    @jobs.command("resume")
    def resume(
        ctx: typer.Context, namespace_id: str, job_id: str, subject_id: str | None = None
    ) -> None:
        invoke(
            ctx,
            lambda api: api.request(
                "POST",
                f"/api/jobs/{segment(job_id)}/resume",
                params={"namespace_id": namespace_id, "subject_id": subject_id},
            ),
        )

    @jobs.command("wait")
    def wait(
        ctx: typer.Context,
        namespace_id: str,
        job_id: str,
        subject_id: str | None = None,
        seconds: float = 120,
    ) -> None:
        invoke(
            ctx,
            lambda api: api.request(
                "POST",
                f"/api/jobs/{segment(job_id)}/wait",
                params={
                    "namespace_id": namespace_id,
                    "subject_id": subject_id,
                    "wait_seconds": seconds,
                },
            ),
        )

    @config.command("list")
    def list_config(ctx: typer.Context, namespace_id: str = "instance") -> None:
        invoke(ctx, lambda api: api.configuration({"action": "list", "namespace_id": namespace_id}))

    @config.command("schema")
    def config_schema(ctx: typer.Context, key: str, namespace_id: str = "instance") -> None:
        invoke(
            ctx,
            lambda api: api.configuration(
                {"action": "schema", "key": key, "namespace_id": namespace_id}
            ),
        )

    @config.command("get")
    def get_config(
        ctx: typer.Context, key: str, namespace_id: str = "instance", subject_id: str | None = None
    ) -> None:
        invoke(
            ctx,
            lambda api: api.request(
                "GET",
                f"/api/configuration/{segment(key)}",
                params={"namespace_id": namespace_id, "subject_id": subject_id},
            ),
        )

    @config.command("history")
    def config_history(
        ctx: typer.Context,
        key: str,
        namespace_id: str = "instance",
        subject_id: str | None = None,
        limit: int = 100,
    ) -> None:
        invoke(
            ctx,
            lambda api: api.configuration(
                {
                    "action": "history",
                    "key": key,
                    "namespace_id": namespace_id,
                    "subject_id": subject_id,
                    "limit": limit,
                }
            ),
        )

    @config.command("set")
    def set_config(
        ctx: typer.Context,
        key: str,
        value: Path,
        namespace_id: str = "instance",
        subject_id: str | None = None,
        scope: str = "instance",
        operation_id: str | None = None,
    ) -> None:
        payload = {
            "value": read_json(value),
            "scope": scope,
            "operation_id": operation_id or str(uuid4()),
        }
        invoke(
            ctx,
            lambda api: api.request(
                "PUT",
                f"/api/configuration/{segment(key)}",
                body=payload,
                params={"namespace_id": namespace_id, "subject_id": subject_id},
            ),
        )

    @prompts.command("list")
    def list_prompts(ctx: typer.Context, namespace_id: str = "instance") -> None:
        invoke(
            ctx,
            lambda api: api.request("GET", "/api/prompts", params={"namespace_id": namespace_id}),
        )

    @prompts.command("get")
    def get_prompt(ctx: typer.Context, prompt_id: str, namespace_id: str = "instance") -> None:
        invoke(
            ctx,
            lambda api: api.request(
                "GET", f"/api/prompts/{segment(prompt_id)}", params={"namespace_id": namespace_id}
            ),
        )

    @prompts.command("history")
    def history(ctx: typer.Context, prompt_id: str, namespace_id: str = "instance") -> None:
        invoke(
            ctx,
            lambda api: api.request(
                "GET",
                f"/api/prompts/{segment(prompt_id)}/history",
                params={"namespace_id": namespace_id},
            ),
        )

    @prompts.command("save")
    def save(
        ctx: typer.Context, prompt_id: str, template: Path, operation_id: str | None = None
    ) -> None:
        payload: dict[str, JsonValue] = {
            "template": template.read_text(encoding="utf-8"),
            "operation_id": operation_id or str(uuid4()),
        }
        invoke(
            ctx, lambda api: api.request("PUT", f"/api/prompts/{segment(prompt_id)}", body=payload)
        )

    @prompts.command("activate")
    def activate(
        ctx: typer.Context, prompt_id: str, revision: int, operation_id: str | None = None
    ) -> None:
        invoke(
            ctx,
            lambda api: api.request(
                "POST",
                f"/api/prompts/{segment(prompt_id)}/activate",
                body={"revision": revision, "operation_id": operation_id or str(uuid4())},
            ),
        )

    @prompts.command("preview")
    def preview(ctx: typer.Context, prompt_id: str, variables: Path) -> None:
        invoke(
            ctx,
            lambda api: api.request(
                "POST",
                f"/api/prompts/{segment(prompt_id)}/preview",
                body={"variables": read_json(variables)},
            ),
        )

    @credentials.command("list")
    def list_credentials(ctx: typer.Context, include_revoked: bool = True) -> None:
        invoke(
            ctx,
            lambda api: api.request(
                "GET", "/api/credentials", params={"include_revoked": include_revoked}
            ),
        )

    @credentials.command("create")
    def create_credential(ctx: typer.Context, command: Path, token_out: Path | None = None) -> None:
        """Create from public CreateCredential JSON; optionally save its one-time token."""
        payload = read_command(command, CreateCredential)
        _issue(ctx, "/api/credentials", encoded(payload), token_out)

    @credentials.command("whoami")
    def whoami(ctx: typer.Context) -> None:
        invoke(ctx, lambda api: api.request("GET", "/api/credentials/whoami"))

    @credentials.command("read")
    def read_credential(ctx: typer.Context, credential_id: str) -> None:
        invoke(ctx, lambda api: api.request("GET", f"/api/credentials/{segment(credential_id)}"))

    @credentials.command("history")
    def history_credential(ctx: typer.Context, credential_id: str) -> None:
        invoke(
            ctx,
            lambda api: api.request("GET", f"/api/credentials/{segment(credential_id)}/history"),
        )

    @credentials.command("rotate")
    def rotate_credential(
        ctx: typer.Context, credential_id: str, command: Path, token_out: Path | None = None
    ) -> None:
        """Replace the token using RotateCredential JSON; save its new one-time value."""
        payload = read_command(command, RotateCredential)
        _issue(
            ctx, f"/api/credentials/{segment(credential_id)}/rotate", encoded(payload), token_out
        )

    @credentials.command("revoke")
    def revoke(
        ctx: typer.Context,
        credential_id: str,
        operation_id: str | None = None,
        expected_revision: int | None = None,
        reason: str = "",
    ) -> None:
        invoke(
            ctx,
            lambda api: api.request(
                "DELETE",
                f"/api/credentials/{segment(credential_id)}",
                params={
                    "operation_id": operation_id or str(uuid4()),
                    "expected_revision": expected_revision,
                    "reason": reason,
                },
            ),
        )

    @cli.command("model-calls")
    def model_calls(ctx: typer.Context, namespace_id: str, subject_id: str | None = None) -> None:
        invoke(
            ctx,
            lambda api: api.request(
                "GET",
                "/api/model-calls",
                params={"namespace_id": namespace_id, "subject_id": subject_id},
            ),
        )

    @cli.command("backup")
    def backup(ctx: typer.Context, output: Path | None = None) -> None:
        """Create an installation backup and optionally stream it to a local file."""
        if output is not None:
            output = output.resolve()
            if output.exists():
                raise FileExistsError(output)

        async def run(api: NousClient) -> object:
            result = await api.request("POST", "/api/backups")
            if output is None:
                return result
            if not isinstance(result, dict) or not isinstance(result.get("filename"), str):
                raise ValueError("Backup response has no filename")
            await download(api, f"/api/backups/{segment(str(result['filename']))}", output)
            return {**result, "output": str(output)}

        invoke(ctx, run)
