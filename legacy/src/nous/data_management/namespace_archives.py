"""Export and restore one Namespace with its bundle and TOML configuration."""

from uuid import NAMESPACE_URL, uuid5

import sqlalchemy as sa
from nous_sdk.contracts import (
    BundleRequest,
    InstallBundle,
    NamespaceBundle,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.identity import ChangeNamespace, SubjectBinding
from nous_sdk.contracts.namespace_archives import (
    NamespaceArchive,
    NamespaceRestoreResult,
    RestoreNamespace,
)
from pydantic import JsonValue, TypeAdapter

from nous.identity.access import require_namespace_in
from nous.identity.schema import subject_bindings
from nous.identity.service import IdentityService, require_management_scope
from nous.platform.configuration.service import ConfigurationService
from nous.platform.persistence import Authority

from .bundles import BundleService


class NamespaceArchiveService:
    def __init__(
        self,
        authority: Authority,
        identity: IdentityService,
        bundles: BundleService,
        configuration: ConfigurationService,
    ) -> None:
        self.authority = authority
        self.identity = identity
        self.bundles = bundles
        self.configuration = configuration

    async def export(self, ctx: CallContext) -> NamespaceArchive:
        require_management_scope(ctx)
        async with self.authority.read(ctx) as view:
            namespace = await require_namespace_in(view, ctx, purpose="management")
            bundle = NamespaceBundle.model_validate(
                await self.bundles.export_in(
                    view, ctx, BundleRequest(), purpose="management"
                )
            )
            bindings = (
                await view.connection.execute(
                    sa.select(subject_bindings).where(
                        subject_bindings.c.namespace_id == ctx.namespace_id
                    )
                )
            ).mappings().all()
            authority_commit_seq = view.commit_seq
        async with self.configuration.write_lock():
            configuration_toml = await self.configuration.export(ctx, "namespace")
        return NamespaceArchive(
            namespace=namespace,
            authority_commit_seq=authority_commit_seq,
            bundle=bundle,
            bindings=tuple(SubjectBinding.model_validate(row) for row in bindings),
            configuration_toml=configuration_toml,
        )

    async def restore(
        self, ctx: CallContext, command: RestoreNamespace
    ) -> NamespaceRestoreResult:
        require_management_scope(ctx)
        if ctx.namespace_id == "instance":
            raise DomainError(
                "namespace_scope_required", "Select a new receiving Namespace identity"
            )
        archive = command.archive
        if archive.bundle.namespace_id != archive.namespace.id:
            raise DomainError("namespace_archive_scope", "Bundle belongs to another Namespace")
        if any(binding.namespace_id != archive.namespace.id for binding in archive.bindings):
            raise DomainError(
                "namespace_archive_scope", "Host binding belongs to another Namespace"
            )

        def step(label: str) -> str:
            return str(
                uuid5(
                    NAMESPACE_URL,
                    f"nous:namespace-restore:{ctx.caller_id}:{ctx.namespace_id}:{command.operation_id}:{label}",
                )
            )

        async with self.configuration.write_lock():
            try:
                await self.identity.namespace(ctx, "management")
            except DomainError as error:
                if error.code != "namespace_not_found":
                    raise
            else:
                raise DomainError("namespace_conflict", "Namespace identity already exists")
            async with self.authority.read(ctx) as view:
                await self.identity.require_fresh_identity_in(view, "namespace", ctx.namespace_id)
            prepared = await self.configuration.prepare_document_locked(
                ctx, archive.configuration_toml, "namespace"
            )
            await self.identity.create_namespace(
                ctx,
                command.slug or archive.namespace.slug,
                step("namespace"),
                namespace_id=ctx.namespace_id,
                display_name=command.display_name or archive.namespace.display_name,
                mode=archive.namespace.mode,
                subject_key=archive.namespace.subject_key,
                enabled_features=archive.namespace.enabled_features,
            )
            await self.identity.change_namespace(
                ctx, ChangeNamespace(operation_id=step("disable"), action="disable")
            )
            installation = await self.bundles.install(
                ctx,
                InstallBundle(
                    bundle=archive.bundle.model_dump(mode="json"),
                    mode="restore",
                ),
                purpose="management",
            )
            async with self.authority.write(ctx) as tx:
                for binding in archive.bindings:
                    value = binding.model_copy(
                        update={"namespace_id": ctx.namespace_id, "commit_seq": tx.commit_seq}
                    )
                    await tx.connection.execute(
                        subject_bindings.insert().values(**value.model_dump())
                    )
            if archive.namespace.state != "disabled":
                await self.identity.change_namespace(
                    ctx,
                    ChangeNamespace(
                        operation_id=step("state"),
                        action="trash" if archive.namespace.state == "trashed" else "enable",
                    ),
                )
            result = NamespaceRestoreResult(
                namespace=await self.identity.namespace(ctx, "management"),
                installation=TypeAdapter(dict[str, JsonValue]).validate_python(installation),
            )
            self.configuration.publish_prepared(prepared)
            return result
