"""Semantic archive export and restoration for one independent cognitive domain."""
from uuid import NAMESPACE_URL, uuid5
import sqlalchemy as sa
from pydantic import JsonValue, TypeAdapter
from nous_sdk.contracts import BundleRequest, CallContext, DomainError, InstallBundle, NamespaceBundle
from nous_sdk.contracts.identity import ChangeNamespace, SubjectBinding
from nous_sdk.contracts.namespace_archives import ArchivedConfiguration, NamespaceArchive, NamespaceRestoreResult, RestoreNamespace
from nous.identity.access import require_namespace_in
from nous.identity.schema import subject_bindings
from nous.identity.service import IdentityService, require_management_scope
from nous.platform.configuration.service import ConfigurationService, configuration
from nous.platform.persistence import Authority
from .bundles import BundleService

class NamespaceArchiveService:
    def __init__(self, authority: Authority, identity: IdentityService, bundles: BundleService, configuration_owner: ConfigurationService):
        self.authority, self.identity, self.bundles, self.configuration = authority, identity, bundles, configuration_owner

    async def export(self, ctx: CallContext) -> NamespaceArchive:
        require_management_scope(ctx)
        async with self.authority.read(ctx) as view:
            namespace = await require_namespace_in(view, ctx, purpose='management')
            bundle = NamespaceBundle.model_validate(await self.bundles.export_in(view, ctx, BundleRequest(), purpose='management'))
            configs = (await view.connection.execute(sa.select(configuration).where(configuration.c.scope == f'namespace:{ctx.namespace_id}'))).mappings().all()
            bindings = (await view.connection.execute(sa.select(subject_bindings).where(subject_bindings.c.namespace_id == ctx.namespace_id))).mappings().all()
            return NamespaceArchive(namespace=namespace, authority_commit_seq=view.commit_seq, bundle=bundle, bindings=tuple(SubjectBinding.model_validate(row) for row in bindings), configuration=tuple(ArchivedConfiguration(key=row['key'], value=row['value']) for row in configs))

    async def restore(self, ctx: CallContext, command: RestoreNamespace) -> NamespaceRestoreResult:
        require_management_scope(ctx)
        if ctx.namespace_id == 'instance': raise DomainError('namespace_scope_required', 'Select a new receiving Namespace identity')
        receipt = await self.authority.receipt(ctx, command.operation_id, operation_kind='namespace.restore_archive', scope=ctx.namespace_id, payload=command)
        if receipt is not None: return NamespaceRestoreResult.model_validate(receipt.result)
        archive = command.archive
        if archive.bundle.namespace_id != archive.namespace.id: raise DomainError('namespace_archive_scope', 'Bundle belongs to another Namespace')
        for entry in archive.configuration:
            definition = self.configuration.entry(entry.key)
            if 'namespace' not in definition.scopes: raise DomainError('configuration_scope_invalid', 'Archived configuration has an invalid scope')
            definition.parse(entry.value)
        def step(label): return str(uuid5(NAMESPACE_URL, f'nous:namespace-restore:{ctx.caller_id}:{ctx.namespace_id}:{command.operation_id}:{label}'))
        await self.identity.create_namespace(ctx, command.slug or archive.namespace.slug, step('namespace'), namespace_id=ctx.namespace_id, display_name=command.display_name or archive.namespace.display_name, mode=archive.namespace.mode, subject_key=archive.namespace.subject_key, enabled_features=archive.namespace.enabled_features)
        await self.identity.change_namespace(ctx, ChangeNamespace(operation_id=step('disable'), action='disable'))
        result = await self.bundles.install(ctx, InstallBundle(operation_id=step('bundle'), bundle=archive.bundle.model_dump(mode='json'), mode='restore'), purpose='management')
        for index, entry in enumerate(archive.configuration): await self.configuration.set(ctx, entry.key, entry.value, step(f'config:{index}'), 'namespace')
        for binding in archive.bindings:
            if binding.namespace_id != archive.namespace.id: raise DomainError('namespace_archive_scope', 'Host binding belongs to another Namespace')
        async with self.authority.write(ctx, step('bindings'), operation_kind='namespace.bindings.restore', scope=ctx.namespace_id, payload=archive.bindings) as tx:
            if not tx.replayed:
                for binding in archive.bindings:
                    await tx.connection.execute(subject_bindings.insert().values(**binding.model_copy(update={'namespace_id':ctx.namespace_id,'commit_seq':tx.commit_seq}).model_dump()))
                tx.result(None)
        if archive.namespace.state != 'disabled': await self.identity.change_namespace(ctx, ChangeNamespace(operation_id=step('state'), action='trash' if archive.namespace.state == 'trashed' else 'enable'))
        value = NamespaceRestoreResult(namespace=await self.identity.namespace(ctx, 'management'), installation=TypeAdapter(dict[str,JsonValue]).validate_python(result))
        async with self.authority.write(ctx, command.operation_id, operation_kind='namespace.restore_archive', scope=ctx.namespace_id, payload=command) as tx:
            return NamespaceRestoreResult.model_validate(tx.replay_result) if tx.replayed else tx.result(value)
