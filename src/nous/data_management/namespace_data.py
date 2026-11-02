"""Permanent Namespace data removal coordinates its existing identity and source owners."""

import tempfile
from pathlib import Path

import sqlalchemy as sa
from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.identity import NamespacePurgeResult, PurgeNamespace

from nous.identity.access import require_namespace_in
from nous.identity.service import IdentityService, require_management_scope
from nous.platform.concurrency import owned_thread
from nous.platform.content import ContentStore
from nous.platform.execution import DurableExecution
from nous.platform.models.ledger import calls
from nous.platform.persistence import Authority, metadata

from .namespace_backups import NamespaceBackupService
from .namespace_state import capture
from .sources import SourceService


class NamespaceDataService:
    def __init__(
        self,
        authority: Authority,
        identity: IdentityService,
        content: ContentStore,
        execution: DurableExecution,
        sources: SourceService,
        backups: NamespaceBackupService,
    ):
        self.authority, self.identity, self.content = authority, identity, content
        self.execution, self.sources, self.backups = execution, sources, backups

    async def purge_namespace(
        self, ctx: CallContext, command: PurgeNamespace
    ) -> NamespacePurgeResult:
        require_management_scope(ctx)
        async with self.identity.namespace_lock(ctx.namespace_id):
            receipt = await self.authority.receipt(
                ctx,
                command.operation_id,
                operation_kind="namespace.purge",
                scope=ctx.namespace_id,
                payload=command,
            )
            if receipt is not None:
                result = NamespacePurgeResult.model_validate(receipt.result)
                await self.content.purge(ctx, result.released_hashes)
                return result
            await self.identity.begin_namespace_purge(
                ctx, command.operation_id, command.expected_revision
            )
            await owned_thread(self.backups.store.initialize)
            with tempfile.TemporaryDirectory(prefix="purge-", dir=self.backups.root) as temporary:
                database = await self.authority.backup(Path(temporary) / "authority.sqlite")
                _, tables, _ = await owned_thread(lambda: capture(database, ctx.namespace_id))
            await self.execution.purge_namespace_jobs(ctx)
            await self.backups.purge_namespace_custody(ctx)
            hashes = tuple(
                dict.fromkeys(
                    str(row["content_hash"])
                    for row in tables.get("nous_content_references", ())
                )
            )
            async with self.authority.write(
                ctx,
                command.operation_id,
                operation_kind="namespace.purge",
                scope=ctx.namespace_id,
                payload=command,
            ) as tx:
                if tx.replayed:
                    result = NamespacePurgeResult.model_validate(tx.replay_result)
                else:
                    current = await require_namespace_in(tx, ctx, purpose="management")
                    if current.state != 'trashed' or not current.purging:
                        raise DomainError(
                            "namespace_not_trashed", "Namespace permanent removal is not admitted"
                        )
                    await self.sources.purge_namespace_in(tx, ctx)
                    deleted = 0
                    for name, rows in tables.items():
                        if name not in metadata.tables or name in {
                            "namespaces",
                            "subject_bindings",
                            "identity_retirements",
                            "nous_model_calls",
                        }:
                            continue
                        table = metadata.tables[name]
                        if "namespace_id" in table.c:
                            statement = table.delete().where(
                                table.c.namespace_id == ctx.namespace_id
                            )
                            deleted += max(0, (await tx.connection.execute(statement)).rowcount)
                        else:
                            keys = tuple(table.primary_key.columns)
                            for row in rows:
                                if not keys:
                                    raise DomainError(
                                        "namespace_purge_integrity",
                                        "Owned data has no stable deletion identity",
                                        table=name,
                                    )
                                statement = table.delete().where(
                                    sa.and_(*(column == row[column.name] for column in keys))
                                )
                                deleted += max(0, (await tx.connection.execute(statement)).rowcount)
                    await tx.connection.execute(
                        calls.update()
                        .where(calls.c.namespace_id == ctx.namespace_id)
                        .values(
                            status="purged",
                            request={"purged": True},
                            result=None,
                            usage=None,
                            error_code="namespace_purged",
                        )
                    )
                    deleted += (
                        len(tables.get("namespaces", ()))
                        + len(tables.get("subject_bindings", ()))
                    )
                    await self.identity.retire_namespace_in(tx, ctx)
                    result = tx.result(
                        NamespacePurgeResult(
                            namespace_id=ctx.namespace_id,
                            commit_seq=tx.commit_seq,
                            deleted_rows=deleted,
                            released_hashes=hashes,
                        )
                    )
            await self.content.purge(ctx, hashes)
            if self.sources.invalidate:
                await self.sources.invalidate()
            return result
