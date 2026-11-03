"""Typed identity management commands use the existing identity SDK."""

import base64
import json
from contextlib import suppress
from pathlib import Path

import anyio
import typer
from nous_sdk import NousClient
from nous_sdk.contracts.identity import (
    BindSubject,
    ChangeNamespace,
    IdentityDirectoryQuery,
    IdentityPurpose,
    PurgeNamespace,
)
from nous_sdk.contracts.namespace_archives import NamespaceArchive, RestoreNamespace
from nous_sdk.contracts.namespace_backups import RestoreNamespaceBackup

from .common import invoke, read_command


def register(namespace: typer.Typer) -> None:
    @namespace.command("purge")
    def purge_namespace(ctx: typer.Context, namespace_id: str, request: Path) -> None:
        command = read_command(request, PurgeNamespace)
        invoke(ctx, lambda api: api.purge_namespace(namespace_id, command))

    @namespace.command("backup")
    def create_backup(ctx: typer.Context, namespace_id: str, output: Path | None = None) -> None:
        target = output.resolve() if output else None

        async def run(api: NousClient) -> object:
            info = await api.create_namespace_backup(namespace_id)
            if target is not None:
                async with await anyio.open_file(target, "xb") as destination:
                    offset = 0
                    while True:
                        chunk = await api.download_namespace_backup(info.backup_id, offset=offset)
                        await destination.write(base64.b64decode(chunk.data_base64, validate=True))
                        offset = chunk.next_offset
                        if chunk.complete:
                            break
            return {**info.model_dump(mode="json"), **({"output": str(target)} if target else {})}

        invoke(ctx, run)

    @namespace.command("backups")
    def list_backups(
        ctx: typer.Context, namespace_id: str, limit: int = 100, cursor: str | None = None
    ) -> None:
        invoke(ctx, lambda api: api.namespace_backups(namespace_id, limit=limit, cursor=cursor))

    @namespace.command("backup-info")
    def backup_info(ctx: typer.Context, backup_id: str) -> None:
        invoke(ctx, lambda api: api.namespace_backup(backup_id))

    @namespace.command("backup-delete")
    def delete_backup(ctx: typer.Context, backup_id: str) -> None:
        invoke(ctx, lambda api: api.delete_namespace_backup(backup_id))

    @namespace.command("backup-upload")
    def upload_backup(ctx: typer.Context, archive: Path) -> None:
        source = archive.resolve(strict=True)

        async def run(api: NousClient) -> object:
            upload = await api.begin_namespace_backup_upload()
            try:
                async with await anyio.open_file(source, "rb") as incoming:
                    while data := await incoming.read(1024 * 1024):
                        upload = await api.upload_namespace_backup_chunk(
                            upload.upload_id, upload.next_offset, data
                        )
                return await api.complete_namespace_backup_upload(upload.upload_id)
            except BaseException:
                with suppress(Exception):
                    await api.abort_namespace_backup_upload(upload.upload_id)
                raise

        invoke(ctx, run)

    @namespace.command("backup-restore")
    def restore_backup(
        ctx: typer.Context,
        namespace_id: str,
        backup_id: str,
        operation_id: str,
        preserve_ids: bool = False,
    ) -> None:
        invoke(
            ctx,
            lambda api: api.restore_namespace_backup(
                namespace_id,
                RestoreNamespaceBackup(
                    backup_id=backup_id, operation_id=operation_id, preserve_ids=preserve_ids
                ),
            ),
        )

    @namespace.command("export")
    def export_namespace(ctx: typer.Context, namespace_id: str, output: Path) -> None:
        output = output.resolve()

        async def run(api: NousClient) -> object:
            value = await api.export_namespace_archive(namespace_id)
            with output.open("x", encoding="utf-8") as target:
                target.write(value.model_dump_json())
            return {
                "output": str(output),
                "authority_commit_seq": value.authority_commit_seq,
            }

        invoke(ctx, run)

    @namespace.command("restore")
    def restore_namespace(
        ctx: typer.Context, namespace_id: str, archive: Path, operation_id: str
    ) -> None:
        command = RestoreNamespace(
            operation_id=operation_id,
            archive=NamespaceArchive.model_validate(
                json.loads(archive.read_text(encoding="utf-8"))
            ),
        )
        invoke(ctx, lambda api: api.restore_namespace_archive(namespace_id, command))

    @namespace.command("get")
    def get_namespace(
        ctx: typer.Context, namespace_id: str, purpose: IdentityPurpose = "active"
    ) -> None:
        invoke(ctx, lambda api: api.get_namespace(namespace_id, purpose=purpose))

    @namespace.command("page")
    def namespace_page(
        ctx: typer.Context,
        limit: int = 100,
        cursor: str | None = None,
        purpose: IdentityPurpose = "active",
    ) -> None:
        invoke(
            ctx,
            lambda api: api.namespace_page(
                IdentityDirectoryQuery(limit=limit, cursor=cursor, purpose=purpose)
            ),
        )

    @namespace.command("change")
    def change_namespace(ctx: typer.Context, namespace_id: str, request: Path) -> None:
        command = read_command(request, ChangeNamespace)
        invoke(ctx, lambda api: api.change_namespace(namespace_id, command))

    @namespace.command("capabilities")
    def namespace_capabilities(
        ctx: typer.Context, namespace_id: str, purpose: IdentityPurpose = "active"
    ) -> None:
        invoke(ctx, lambda api: api.identity_capabilities(namespace_id, purpose=purpose))

    @namespace.command("bind-subject")
    def bind_subject(ctx: typer.Context, namespace_id: str, request: Path) -> None:
        command = read_command(request, BindSubject)
        invoke(ctx, lambda api: api.bind_subject(namespace_id, command))

    @namespace.command("subject-binding")
    def subject_binding(
        ctx: typer.Context, namespace_id: str, external_id: str, purpose: IdentityPurpose = "active"
    ) -> None:
        invoke(ctx, lambda api: api.subject_binding(namespace_id, external_id, purpose=purpose))
