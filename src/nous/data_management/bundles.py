from __future__ import annotations

from nous_wave.contracts import (
    CallContext,
    DomainError,
    SourceCommand,
)
from sqlalchemy import select, update

from nous.cognition.service import CognitionService
from nous.identity.service import require_subject_in
from nous.platform.content import ContentStore
from nous.platform.persistence import Authority

from . import bundle_export, bundle_install
from .schema import bundle_installs


class BundleService:
    export = bundle_export.export
    export_zip = bundle_export.export_zip
    inspect_zip = bundle_export.inspect_zip
    install = bundle_install.install
    install_zip = bundle_install.install_zip
    install_prepared = bundle_install.install_prepared

    def __init__(
        self,
        authority: Authority,
        cognition: CognitionService,
        content_store: ContentStore | None = None,
    ) -> None:
        self.authority, self.cognition = authority, cognition
        self.content_store = content_store

    async def list_installs(self, ctx: CallContext):
        async with self.authority.read(ctx) as view:
            await require_subject_in(view, ctx)
            return [
                row
                for row in (
                    await view.connection.execute(
                        select(bundle_installs.c.record).where(
                            bundle_installs.c.namespace_id == ctx.namespace_id,
                            bundle_installs.c.subject_id == ctx.subject_id,
                        )
                    )
                ).scalars()
            ]

    async def uninstall(self, ctx: CallContext, installation_id: str, operation_id: str):
        from .sources import SourceService

        sources = SourceService(self.authority, self.cognition)
        async with self.authority.write(
            ctx,
            operation_id,
            operation_kind="bundle.uninstall",
            scope=ctx.require_subject(),
            payload={"installation_id": installation_id},
        ) as tx:
            if tx.replayed:
                return tx.replay_result
            value = (
                await tx.connection.execute(
                    select(bundle_installs.c.record).where(
                        bundle_installs.c.id == installation_id,
                        bundle_installs.c.namespace_id == ctx.namespace_id,
                        bundle_installs.c.subject_id == ctx.subject_id,
                    )
                )
            ).scalar_one_or_none()
            if value is None:
                raise DomainError("bundle_install_not_found", "Bundle installation is unavailable")
            if value.get("state") == "uninstalled":
                return tx.result(
                    {
                        "installation_id": installation_id,
                        "state": "uninstalled",
                        "already_uninstalled": True,
                    }
                )
            owned = tuple(value.get("owned_observation_ids", ()))
            affected: list[str] = []
            if owned:
                result, _detached = await sources.change_in(
                    tx,
                    ctx,
                    SourceCommand(
                        operation_id=operation_id,
                        observation_ids=owned,
                        reason=f"uninstall bundle {value['bundle_id']}",
                    ),
                    "trash",
                )
                affected = [ref.exact() for ref in result.affected]
            value = {
                **value,
                "state": "uninstalled",
                "uninstall_operation_id": operation_id,
                "uninstalled_commit_seq": tx.commit_seq,
            }
            await tx.connection.execute(
                update(bundle_installs)
                .where(bundle_installs.c.id == installation_id)
                .values(record=value)
            )
            return tx.result(
                {
                    "installation_id": installation_id,
                    "state": "uninstalled",
                    "affected": affected,
                    "commit_seq": tx.commit_seq,
                }
            )
