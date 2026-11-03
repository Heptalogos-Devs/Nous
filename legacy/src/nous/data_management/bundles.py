from __future__ import annotations

from nous_sdk.contracts import (
    SourceCommand,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from sqlalchemy import select, update

from nous.cognition.records import CognitiveRecords
from nous.identity.access import require_namespace_in
from nous.platform.content import ContentStore
from nous.platform.persistence import Authority

from . import archive, bundle_export, bundle_install
from .schema import bundle_installs
from .sources import SourceService


class BundleService:
    export_archive = archive.export_archive
    export = bundle_export.export
    export_in = bundle_export.export_in
    export_zip = bundle_export.export_zip
    inspect_zip = bundle_export.inspect_zip
    install = bundle_install.install
    install_zip = bundle_install.install_zip
    install_prepared = bundle_install.install_prepared

    def __init__(
        self,
        authority: Authority,
        cognition: CognitiveRecords,
        *,
        sources: SourceService,
        content_store: ContentStore | None = None,
    ) -> None:
        self.authority, self.cognition = authority, cognition
        self.content_store = content_store
        self.sources = sources

    async def list_installs(self, ctx: CallContext):
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            return [
                row
                for row in (
                    await view.connection.execute(
                        select(bundle_installs.c.record).where(
                            bundle_installs.c.namespace_id == ctx.namespace_id,
                        )
                    )
                ).scalars()
            ]

    async def uninstall(self, ctx: CallContext, installation_id: str):
        async with self.authority.write(ctx) as tx:
            value = (
                await tx.connection.execute(
                    select(bundle_installs.c.record).where(
                        bundle_installs.c.id == installation_id,
                        bundle_installs.c.namespace_id == ctx.namespace_id,
                    )
                )
            ).scalar_one_or_none()
            if value is None:
                raise DomainError("bundle_install_not_found", "Bundle installation is unavailable")
            if value.get("state") == "uninstalled":
                return {'installation_id': installation_id, 'state': 'uninstalled', 'already_uninstalled': True}
            owned = tuple(value.get("owned_observation_ids", ()))
            affected: list[str] = []
            if owned:
                result, _detached = await self.sources.change_in(
                    tx,
                    ctx,
                    SourceCommand(
                        observation_ids=owned,
                        reason=f"uninstall bundle {value['bundle_id']}",
                    ),
                    "trash",
                )
                affected = [ref.exact() for ref in result.affected]
            value = {
                **value,
                "state": "uninstalled",
                "uninstalled_commit_seq": tx.commit_seq,
            }
            await tx.connection.execute(
                update(bundle_installs)
                .where(bundle_installs.c.id == installation_id)
                .values(record=value)
            )
            return {'installation_id': installation_id, 'state': 'uninstalled', 'affected': affected, 'commit_seq': tx.commit_seq}

    async def get_installation(self, ctx: CallContext, installation_id: str):
        values = await self.list_installs(ctx)
        for value in values:
            if value["installation_id"] == installation_id:
                return value
        raise DomainError("bundle_install_not_found", "Bundle installation is unavailable")
