"""Namespace lifecycle, feature presets and stable host bindings."""

import asyncio
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager
from uuid import uuid4
import sqlalchemy as sa
from sqlalchemy.exc import IntegrityError
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.identity import (
    BindSubject,
    BoundSubject,
    ChangeNamespace,
    IdentityCapabilities,
    IdentityDirectoryQuery,
    IdentityPurpose,
    Namespace,
    NamespacePage,
    SubjectBinding,
    SUPPORTED_FEATURES,
)
from nous.platform.persistence import Authority, ReadView, WriteTx
from .access import require_namespace_in
from .schema import namespaces, retired, subject_bindings

SUPPORTED_CAPABILITIES = SUPPORTED_FEATURES


def require_management_scope(ctx: CallContext) -> None:
    if ctx.source_groups():
        raise DomainError(
            "identity_management_scope_forbidden",
            "Identity management requires the complete target scope",
        )


class IdentityService:
    def __init__(self, authority: Authority) -> None:
        self.authority = authority
        self._namespace_locks: dict[str, asyncio.Lock] = {}

    @asynccontextmanager
    async def namespace_lock(self, namespace_id: str) -> AsyncGenerator[None]:
        async with self._namespace_locks.setdefault(namespace_id, asyncio.Lock()):
            yield

    async def require_fresh_identity_in(self, view: ReadView | WriteTx, kind: str, identifier: str) -> None:
        if await view.connection.scalar(
            sa.select(retired.c.id).where(retired.c.kind == kind, retired.c.id == identifier)
        ):
            raise DomainError(
                "identity_purged", "A permanently removed identity cannot be reused", id=identifier
            )

    async def create_namespace(
        self,
        ctx: CallContext,
        slug: str,
        operation_id: str,
        namespace_id: str | None = None,
        *,
        display_name: str | None = None,
        mode: str = "knowledge",
        subject_key: str | None = None,
        enabled_features: tuple[str, ...] | None = None,
    ) -> Namespace:
        require_management_scope(ctx)
        dict(
            slug=slug,
            namespace_id=namespace_id,
            display_name=display_name,
            mode=mode,
            subject_key=subject_key,
            enabled_features=enabled_features,
        )
        async with self.authority.write(ctx) as tx:
            return await self.create_namespace_in(tx, ctx, slug, namespace_id, display_name=display_name, mode=mode, subject_key=subject_key, enabled_features=enabled_features)

    async def create_namespace_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        slug: str,
        namespace_id: str | None = None,
        *,
        display_name: str | None = None,
        mode: str = "knowledge",
        subject_key: str | None = None,
        enabled_features: tuple[str, ...] | None = None,
    ) -> Namespace:
        require_management_scope(ctx)
        if tx.authority is not self.authority:
            raise ValueError("Identity creation requires its owning live transaction")
        identifier = namespace_id or (
            ctx.namespace_id if ctx.namespace_id != "instance" else str(uuid4())
        )
        await self.require_fresh_identity_in(tx, "namespace", identifier)
        if await tx.connection.scalar(
            sa.select(namespaces.c.id).where(
                sa.or_(
                    namespaces.c.id == identifier,
                    namespaces.c.slug == slug,
                    namespaces.c.subject_key == subject_key if subject_key else sa.false(),
                )
            )
        ):
            raise DomainError(
                "namespace_conflict", "Namespace identity, slug or subject key already exists"
            )
        from nous_sdk.contracts.identity import CreateNamespace

        command = CreateNamespace(
            slug=slug,
            display_name=display_name,
            mode=mode,
            subject_key=subject_key,
            enabled_features=enabled_features,
        )
        value = Namespace(
            id=identifier,
            slug=command.slug,
            display_name=command.display_name or command.slug,
            mode=command.mode,
            subject_key=command.subject_key,
            enabled_features=command.enabled_features
            if command.enabled_features is not None
            else (
                SUPPORTED_FEATURES
                if command.mode == "subject"
                else ("memory", "episodes", "person_profiles")
            ),
            commit_seq=tx.commit_seq,
            created_at=tx.recorded_at,
            updated_at=tx.recorded_at,
        )
        await tx.connection.execute(namespaces.insert().values(**value.model_dump()))
        return value

    async def namespace(self, ctx: CallContext, purpose: IdentityPurpose = "active") -> Namespace:
        if purpose == "management":
            require_management_scope(ctx)
        async with self.authority.read(ctx) as view:
            return await require_namespace_in(view, ctx, purpose=purpose)

    async def resolve(self, name_or_id: str, *, purpose: IdentityPurpose = "active") -> Namespace:
        async with self.authority.read() as view:
            identifier = await view.connection.scalar(
                sa.select(namespaces.c.id).where(
                    sa.or_(namespaces.c.id == name_or_id, namespaces.c.slug == name_or_id)
                )
            )
            if identifier is None:
                raise DomainError("namespace_not_found", "Namespace does not exist")
            ctx = CallContext(
                caller_id="namespace-resolver", namespace_id=identifier, credential_source_ids=None
            )
            return await require_namespace_in(view, ctx, purpose=purpose)

    async def namespace_page(
        self,
        ctx: CallContext,
        command: IdentityDirectoryQuery,
        *,
        allowed_namespaces: tuple[str, ...] | None = None,
    ) -> NamespacePage:
        if command.purpose == "management":
            require_management_scope(ctx)
        statement = sa.select(namespaces)
        if command.purpose == "active":
            statement = statement.where(
                namespaces.c.state == "active", namespaces.c.purging.is_(False)
            )
        elif command.states:
            statement = statement.where(namespaces.c.state.in_(command.states))
        if allowed_namespaces is not None:
            statement = statement.where(namespaces.c.id.in_(allowed_namespaces))
        if command.subject_key:
            statement = statement.where(namespaces.c.subject_key == command.subject_key)
        if command.cursor:
            statement = statement.where(namespaces.c.id > command.cursor)
        async with self.authority.read(ctx) as view:
            rows = (
                (
                    await view.connection.execute(
                        statement.order_by(namespaces.c.id).limit(command.limit + 1)
                    )
                )
                .mappings()
                .all()
            )
        values = tuple(Namespace.model_validate(row) for row in rows[: command.limit])
        return NamespacePage(
            items=values, next_cursor=values[-1].id if len(rows) > command.limit else None
        )

    async def capabilities(
        self, ctx: CallContext, purpose: IdentityPurpose = "active"
    ) -> IdentityCapabilities:
        value = await self.namespace(ctx, purpose)
        return IdentityCapabilities(
            namespace_id=value.id,
            supported=SUPPORTED_FEATURES,
            configured=value.enabled_features,
            enabled=value.enabled_features if value.state == "active" and not value.purging else (),
        )

    async def change_namespace(self, ctx: CallContext, command: ChangeNamespace) -> Namespace:
        require_management_scope(ctx)
        async with self.authority.write(ctx) as tx:
            current = await require_namespace_in(tx, ctx, purpose="management")
            if current.purging:
                raise DomainError("namespace_purging", "Namespace permanent removal is in progress")
            if (
                command.expected_revision is not None
                and current.revision != command.expected_revision
            ):
                raise DomainError(
                    "identity_revision_conflict",
                    "Namespace revision changed",
                    actual_revision=current.revision,
                )
            if current.state == "trashed" and command.action != "restore":
                raise DomainError("identity_deleted", "Restore this Namespace before changing it")
            changes = dict(
                revision=current.revision + 1, commit_seq=tx.commit_seq, updated_at=tx.recorded_at
            )
            if command.action == "rename":
                if command.slug is None and command.display_name is None:
                    raise DomainError("identity_name_required", "Supply a slug or display name")
                if command.slug is not None:
                    changes["slug"] = command.slug
                if command.display_name is not None:
                    changes["display_name"] = command.display_name
            elif command.action == "configure_features":
                if command.enabled_features is None:
                    raise DomainError("features_required", "Supply the enabled feature set")
                changes["enabled_features"] = command.enabled_features
            else:
                changes["state"] = {
                    "enable": "active",
                    "disable": "disabled",
                    "trash": "trashed",
                    "restore": "active",
                }[command.action]
            value = Namespace.model_validate({**current.model_dump(), **changes})
            try:
                await tx.connection.execute(
                    namespaces.update().where(namespaces.c.id == current.id).values(**changes)
                )
            except IntegrityError as error:
                raise DomainError("namespace_conflict", "Namespace slug already exists") from error
            return value

    async def begin_namespace_purge(
        self, ctx: CallContext, operation_id: str, expected_revision: int | None
    ) -> Namespace:
        require_management_scope(ctx)
        async with self.authority.write(ctx) as tx:
            current = await require_namespace_in(tx, ctx, purpose="management")
            if current.state != "trashed":
                raise DomainError(
                    "namespace_not_trashed",
                    "Move the Namespace to the recycle bin before permanent removal",
                )
            if (
                expected_revision is not None
                and expected_revision != current.revision
                and not current.purging
            ):
                raise DomainError(
                    "identity_revision_conflict",
                    "Namespace revision changed",
                    actual_revision=current.revision,
                )
            value = current.model_copy(
                update=dict(
                    purging=True,
                    revision=current.revision + 1,
                    commit_seq=tx.commit_seq,
                    updated_at=tx.recorded_at,
                )
            )
            await tx.connection.execute(
                namespaces.update().where(namespaces.c.id == value.id).values(**value.model_dump())
            )
            return value

    async def retire_namespace_in(self, tx: WriteTx, ctx: CallContext) -> None:
        require_management_scope(ctx)
        current = await require_namespace_in(tx, ctx, purpose="management")
        if current.state != "trashed" or not current.purging:
            raise DomainError(
                "namespace_not_trashed", "Namespace permanent removal has not been admitted"
            )
        await tx.connection.execute(
            retired.insert().values(
                kind="namespace",
                id=current.id,
                namespace_id=current.id,
                purged_at=tx.recorded_at,
                commit_seq=tx.commit_seq,
            )
        )
        await tx.connection.execute(
            subject_bindings.delete().where(subject_bindings.c.namespace_id == current.id)
        )
        await tx.connection.execute(namespaces.delete().where(namespaces.c.id == current.id))

    async def bind(self, ctx: CallContext, command: BindSubject) -> BoundSubject:
        require_management_scope(ctx)
        host = command.host_id or ctx.caller_id
        if host != ctx.caller_id:
            raise DomainError(
                "host_binding_forbidden", "Host bindings are owned by the authenticated caller"
            )
        async with self.authority.write(ctx) as tx:
            namespace = await require_namespace_in(tx, ctx)
            row = (
                (
                    await tx.connection.execute(
                        sa.select(subject_bindings).where(
                            subject_bindings.c.host_id == host,
                            subject_bindings.c.external_id == command.external_id,
                        )
                    )
                )
                .mappings()
                .first()
            )
            if row:
                binding = SubjectBinding.model_validate(row)
                if binding.namespace_id != ctx.namespace_id:
                    raise DomainError(
                        "subject_binding_conflict",
                        "Stable host identity is already bound to another Namespace",
                    )
                result = BoundSubject(binding=binding, namespace=namespace)
                return result
            binding = SubjectBinding(
                namespace_id=namespace.id,
                host_id=host,
                external_id=command.external_id,
                commit_seq=tx.commit_seq,
            )
            await tx.connection.execute(subject_bindings.insert().values(**binding.model_dump()))
            return BoundSubject(binding=binding, namespace=namespace)

    async def binding(
        self, ctx: CallContext, external_id: str, purpose: IdentityPurpose = "active"
    ) -> BoundSubject:
        async with self.authority.read(ctx) as view:
            namespace = await require_namespace_in(view, ctx, purpose=purpose)
            row = (
                (
                    await view.connection.execute(
                        sa.select(subject_bindings).where(
                            subject_bindings.c.host_id == ctx.caller_id,
                            subject_bindings.c.external_id == external_id,
                            subject_bindings.c.namespace_id == ctx.namespace_id,
                        )
                    )
                )
                .mappings()
                .first()
            )
            if row is None:
                raise DomainError("subject_binding_not_found", "Host binding does not exist")
            return BoundSubject(binding=SubjectBinding.model_validate(row), namespace=namespace)

    async def restore_directory_in(
        self, tx: WriteTx, namespace: Namespace, bindings: tuple[SubjectBinding, ...] = ()
    ) -> None:
        require_management_scope(tx.ctx)
        if namespace.id != tx.ctx.namespace_id or namespace.id == "instance":
            raise DomainError(
                "namespace_backup_scope_forbidden",
                "Identity restore requires its complete receiving Namespace",
            )
        await self.require_fresh_identity_in(tx, "namespace", namespace.id)
        await tx.connection.execute(namespaces.insert().values(**namespace.model_dump()))
        for binding in bindings:
            if binding.namespace_id != namespace.id:
                raise DomainError(
                    "namespace_backup_integrity", "Host binding belongs to another Namespace"
                )
            await tx.connection.execute(subject_bindings.insert().values(**binding.model_dump()))
