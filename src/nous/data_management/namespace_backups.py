"""Namespace backups own scoped snapshot custody and resumable archive transfers."""

import base64
import shutil
import tempfile
from pathlib import Path
from uuid import uuid4

import sqlalchemy as sa
from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.namespace_backups import (
    NamespaceBackupChunk,
    NamespaceBackupInfo,
    NamespaceBackupRestored,
    NamespaceBackupsPage,
    NamespaceBackupUpload,
    NamespaceState,
    RestoreNamespaceBackup,
)
from pydantic import TypeAdapter

from nous.identity.service import IdentityService, require_management_scope
from nous.platform.concurrency import owned_thread
from nous.platform.content import ContentLease, ContentStore
from nous.platform.persistence import Authority, metadata
from nous.platform.execution import DurableExecution

from .namespace_backup_store import NamespaceBackupStore
from .namespace_state import capture

uploads = sa.Table(
    "namespace_backup_uploads",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("caller_id", sa.String, nullable=False),
    sa.Column("namespace_id", sa.String, nullable=False),
)


class NamespaceBackupService:
    def __init__(
        self, authority: Authority, identity: IdentityService, content: ContentStore, root: Path, *, execution: DurableExecution
    ):
        self.authority, self.identity, self.content = authority, identity, content
        self.execution = execution
        self.root = root
        self.store = NamespaceBackupStore(root / "archives", root / "uploads")

    async def namespace_for_backup(self, backup_id: str) -> str:
        return (await owned_thread(lambda: self.store.get_backup(backup_id))).source_namespace_id

    async def namespace_for_upload(self, upload_id: str) -> str:
        async with self.authority.read() as view:
            namespace_id = await view.connection.scalar(
                sa.select(uploads.c.namespace_id).where(uploads.c.id == upload_id)
            )
        if namespace_id is None:
            raise DomainError("namespace_backup_upload_not_found", "Backup upload does not exist")
        return str(namespace_id)

    async def purge_namespace_custody(self, ctx: CallContext) -> None:
        self.require_scope(ctx)
        while True:
            page = await self.list(ctx, limit=1000)
            if not page.items:
                break
            for info in page.items:
                await self.delete(ctx, info.backup_id)
        async with self.authority.read(ctx) as view:
            pending = [
                str(value)
                for value in (
                    await view.connection.execute(
                        sa.select(uploads.c.id).where(uploads.c.namespace_id == ctx.namespace_id)
                    )
                ).scalars()
            ]
        for upload_id in pending:
            await owned_thread(lambda identifier=upload_id: self.store.abort_upload(identifier))
        if pending:
            async with self.authority.write(
                ctx,
                str(uuid4()),
                operation_kind="namespace.backup.upload.purge",
                scope=ctx.namespace_id,
                payload={},
            ) as tx:
                await tx.connection.execute(uploads.delete().where(uploads.c.id.in_(pending)))
                tx.result(None)

    @staticmethod
    def require_scope(ctx: CallContext, source_namespace_id: str | None = None) -> None:
        require_management_scope(ctx)
        if source_namespace_id is not None and ctx.namespace_id not in {
            "instance",
            source_namespace_id,
        }:
            raise DomainError(
                "namespace_backup_scope_forbidden", "Backup belongs to another namespace"
            )

    async def create(self, ctx: CallContext) -> NamespaceBackupInfo:
        async with self.identity.namespace_lock(ctx.namespace_id):
            self.require_scope(ctx)
            namespace = await self.identity.namespace(ctx, "management")
            if namespace.purging:
                raise DomainError("namespace_purging", "Namespace permanent removal is in progress")
            await owned_thread(self.store.initialize)
            with tempfile.TemporaryDirectory(prefix="snapshot-", dir=self.root) as temporary:
                temporary_path = Path(temporary)
                async with self.content.preserve_all():
                    database = await self.authority.backup(temporary_path / "authority.sqlite")
                    commit, tables, workflow_ids = await owned_thread(lambda: capture(database, ctx.namespace_id))
                    snapshot_authority = Authority(database)
                    try:
                        async with snapshot_authority.read(ctx) as view:
                            execution = await self.execution.export_namespace_in(view, ctx, workflow_ids=workflow_ids)
                    finally:
                        await snapshot_authority.close()
                    state = NamespaceState(namespace_id=ctx.namespace_id, authority_commit_seq=commit, tables=tables, execution=execution)
                    namespace = await self.identity.namespace(ctx, "management")
                    source = temporary_path / "namespace"
                    await owned_thread(lambda: self._stage(state, source))
                    version = max(
                        (
                            TypeAdapter(int).validate_python(row["commit_seq"])
                            for row in state.tables.get("nous_configuration_history", ())
                        ),
                        default=0,
                    )
                    return await owned_thread(
                        lambda: self.store.create_backup(
                            namespace, source, source_config_version=version
                        )
                    )

    def _stage(self, state: NamespaceState, target: Path) -> None:
        target.mkdir()
        (target / "namespace.json").write_text(state.model_dump_json(), encoding="utf-8")
        for row in state.tables.get("nous_content_references", ()):
            digest = str(row["content_hash"])
            destination = target / "objects" / digest[:2] / digest[2:]
            if destination.exists():
                continue
            destination.parent.mkdir(parents=True, exist_ok=True)
            source = self.content.path_for(digest)
            if source.stat().st_size != row["byte_len"]:
                raise DomainError(
                    "namespace_backup_integrity",
                    "Content length differs from its authoritative reference",
                )
            shutil.copyfile(source, destination)

    async def get(self, ctx: CallContext, backup_id: str) -> NamespaceBackupInfo:
        self.require_scope(ctx)
        info = await owned_thread(lambda: self.store.get_backup(backup_id))
        self.require_scope(ctx, info.source_namespace_id)
        return info

    async def list(
        self, ctx: CallContext, limit: int = 100, cursor: str | None = None
    ) -> NamespaceBackupsPage:
        self.require_scope(ctx)
        if not 1 <= limit <= 1000:
            raise ValueError("Backup page limit must be between 1 and 1000")
        values = await owned_thread(
            lambda: self.store.list_backups(
                "" if ctx.namespace_id == "instance" else ctx.namespace_id
            )
        )
        if cursor:
            position = next(
                (index + 1 for index, item in enumerate(values) if item.backup_id == cursor), None
            )
            if position is None:
                raise ValueError("Backup cursor is unavailable")
            values = values[position:]
        page = values[:limit]
        return NamespaceBackupsPage(
            items=tuple(page), next_cursor=page[-1].backup_id if len(values) > limit else None
        )

    async def list_all(
        self, ctx: CallContext, limit: int = 100, cursor: str | None = None
    ) -> NamespaceBackupsPage:
        if ctx.namespace_id != "instance":
            raise DomainError(
                "namespace_backup_scope_forbidden",
                "Complete backup directory requires instance management scope",
            )
        return await self.list(ctx, limit, cursor)

    async def delete(self, ctx: CallContext, backup_id: str) -> None:
        await self.get(ctx, backup_id)
        await owned_thread(lambda: self.store.delete_backup(backup_id))

    async def download(
        self, ctx: CallContext, backup_id: str, offset: int = 0, max_bytes: int = 262144
    ) -> NamespaceBackupChunk:
        await self.get(ctx, backup_id)
        info, data, next_offset, complete = await owned_thread(
            lambda: self.store.read_chunk(backup_id, offset=offset, max_bytes=max_bytes)
        )
        return NamespaceBackupChunk(
            backup=info,
            offset=offset,
            data_base64=base64.b64encode(data).decode("ascii"),
            next_offset=next_offset,
            complete=complete,
        )

    async def begin_upload(self, ctx: CallContext) -> NamespaceBackupUpload:
        self.require_scope(ctx)
        upload = await owned_thread(self.store.begin_upload)
        try:
            async with self.authority.write(
                ctx,
                upload.upload_id,
                operation_kind="namespace.backup.upload.begin",
                scope="namespace-backups",
                payload={},
            ) as tx:
                await tx.connection.execute(
                    uploads.insert().values(
                        id=upload.upload_id, caller_id=ctx.caller_id, namespace_id=ctx.namespace_id
                    )
                )
                tx.result(upload)
            return upload
        except BaseException:
            await owned_thread(lambda: self.store.abort_upload(upload.upload_id))
            raise

    async def _upload(self, ctx: CallContext, upload_id: str) -> None:
        self.require_scope(ctx)
        async with self.authority.read(ctx) as view:
            row = (
                (await view.connection.execute(sa.select(uploads).where(uploads.c.id == upload_id)))
                .mappings()
                .first()
            )
            if row is None:
                raise DomainError(
                    "namespace_backup_upload_not_found", "Backup upload does not exist"
                )
            if row["caller_id"] != ctx.caller_id or row["namespace_id"] != ctx.namespace_id:
                raise DomainError(
                    "namespace_backup_upload_forbidden",
                    "Backup upload belongs to another caller or namespace",
                )

    async def upload_chunk(
        self, ctx: CallContext, upload_id: str, offset: int, data: bytes
    ) -> NamespaceBackupUpload:
        await self._upload(ctx, upload_id)
        return await owned_thread(
            lambda: self.store.append_upload(upload_id, offset=offset, data=data)
        )

    async def complete_upload(
        self, ctx: CallContext, upload_id: str, expected_sha256: str = ""
    ) -> NamespaceBackupInfo:
        await self._upload(ctx, upload_id)
        info = await owned_thread(
            lambda: self.store.complete_upload(
                upload_id,
                expected_sha256=expected_sha256,
                source_namespace_id=None if ctx.namespace_id == "instance" else ctx.namespace_id,
            )
        )
        await self._end_upload(ctx, upload_id)
        return info

    async def _end_upload(self, ctx: CallContext, upload_id: str) -> None:
        async with self.authority.write(
            ctx,
            upload_id,
            operation_kind="namespace.backup.upload.end",
            scope="namespace-backups",
            payload={},
        ) as tx:
            if not tx.replayed:
                await tx.connection.execute(uploads.delete().where(uploads.c.id == upload_id))
                tx.result(None)

    async def abort_upload(self, ctx: CallContext, upload_id: str) -> None:
        await self._upload(ctx, upload_id)
        await owned_thread(lambda identifier=upload_id: self.store.abort_upload(identifier))
        await self._end_upload(ctx, upload_id)

    async def restore(
        self, ctx: CallContext, command: RestoreNamespaceBackup
    ) -> NamespaceBackupRestored:
        from .namespace_restore import restore

        self.require_scope(ctx)
        await owned_thread(self.store.initialize)
        backup_info = await owned_thread(lambda: self.store.get_backup(command.backup_id))
        with tempfile.TemporaryDirectory(prefix="restore-", dir=self.root) as temporary:
            target = Path(temporary)
            await owned_thread(lambda: self.store.extract_backup(command.backup_id, target))
            state = await owned_thread(
                lambda: NamespaceState.model_validate_json((target / "namespace.json").read_bytes())
            )
            if state.namespace_id != backup_info.source_namespace_id:
                raise DomainError(
                    "namespace_backup_integrity",
                    "Namespace state does not match the backup manifest",
                )
            leases: dict[str, ContentLease] = {}
            try:
                for row in state.tables.get("nous_content_references", ()):
                    digest = str(row["content_hash"])
                    if digest not in leases:
                        lease = await self.content.publish_file(
                            target / "objects" / digest[:2] / digest[2:], str(row["media_type"])
                        )
                        if lease.content_hash != digest or lease.byte_len != row["byte_len"]:
                            await self.content.release(lease)
                            raise DomainError(
                                "namespace_backup_integrity",
                                "Restored content differs from its authoritative reference",
                            )
                        leases[digest] = lease
                return await restore(
                    self.authority, self.identity, self.content, self.execution, ctx, command, state, leases
                )
            finally:
                for lease in leases.values():
                    await self.content.release(lease)
