"""Opaque API tokens and centralized trusted caller scope attenuation."""

from __future__ import annotations

import hashlib
import hmac
import re
import secrets
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Literal
from uuid import uuid4

import sqlalchemy as sa
from nous_wave.contracts.common import CallContext, DomainError
from nous_wave.contracts.credentials import (
    Capability,
    CreateCredential,
    CredentialEvent,
    CredentialRecord,
    IssuedCredential,
    PrincipalRecord,
    RotateCredential,
)
from sqlalchemy.ext.asyncio import AsyncConnection

from nous.platform.concurrency import owned_thread
from nous.platform.persistence import Authority, WriteTx, metadata

credentials = sa.Table(
    "nous_api_credentials",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("token_hash", sa.String, unique=True, nullable=False),
    sa.Column("record", sa.JSON, nullable=False),
)
credential_events = sa.Table(
    "nous_credential_events",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("credential_id", sa.String, nullable=False, index=True),
    sa.Column("revision", sa.Integer, nullable=False),
    sa.Column("commit_seq", sa.Integer, nullable=False),
    sa.Column("record", sa.JSON, nullable=False),
    sa.UniqueConstraint("credential_id", "revision", name="uq_credential_event_revision"),
)


class AuthenticationError(DomainError):
    def __init__(self) -> None:
        super().__init__("credential_invalid", "Credential is invalid or revoked")


def _scope_subset(child: tuple[str, ...] | None, parent: tuple[str, ...] | None) -> bool:
    return parent is None or (child is not None and set(child).issubset(parent))


@dataclass(frozen=True)
class Principal:
    caller_id: str
    namespace_ids: tuple[str, ...] | None = None
    subject_ids: tuple[str, ...] | None = None
    source_ids: tuple[str, ...] | None = None
    capabilities: frozenset[Capability] = frozenset()
    credential_id: str | None = None

    def require(self, capability: Capability) -> None:
        if capability not in self.capabilities:
            raise DomainError(
                "capability_forbidden", f"Credential does not grant {capability} access"
            )

    def require_scope(self, namespace_id: str, subject_id: str | None = None) -> None:
        if self.namespace_ids is not None and namespace_id not in self.namespace_ids:
            raise DomainError(
                "namespace_scope_forbidden", "Credential does not grant this namespace"
            )
        if self.subject_ids is not None and (
            subject_id is None or subject_id not in self.subject_ids
        ):
            raise DomainError("subject_scope_forbidden", "Credential requires a permitted subject")

    def require_instance_management(self) -> None:
        self.require("manage")
        if any(
            scope is not None for scope in (self.namespace_ids, self.subject_ids, self.source_ids)
        ):
            raise DomainError(
                "instance_scope_forbidden",
                "This operation requires unrestricted instance management",
            )

    def intersect_sources(self, requested: tuple[str, ...] | None) -> tuple[str, ...] | None:
        """Literal identity intersection; resource containment belongs to source_scope."""
        if self.source_ids is None:
            return requested
        if requested is None:
            return self.source_ids
        return tuple(value for value in dict.fromkeys(requested) if value in self.source_ids)

    def permits_record(self, record: CredentialRecord) -> bool:
        return (
            _scope_subset(record.namespace_ids, self.namespace_ids)
            and _scope_subset(record.subject_ids, self.subject_ids)
            and _scope_subset(record.source_ids, self.source_ids)
        )


class CredentialsService:
    def __init__(self, authority: Authority, bootstrap_token_file: str | Path):
        self.authority = authority
        self.bootstrap_token_file = Path(bootstrap_token_file)

    def _bootstrap_token(self) -> str | None:
        try:
            value = self.bootstrap_token_file.read_text(encoding="utf-8").strip()
        except FileNotFoundError:
            return None
        return value or None

    async def authenticate(self, token: str) -> Principal:
        if not token or len(token) > 1024:
            raise AuthenticationError()
        bootstrap = await owned_thread(self._bootstrap_token)
        if bootstrap is not None and hmac.compare_digest(token.encode(), bootstrap.encode()):
            return Principal(
                caller_id="instance-admin", capabilities=frozenset(("read", "write", "manage"))
            )
        if re.fullmatch(r"nous_[0-9a-f]{32}_[A-Za-z0-9_-]{43}", token) is None:
            raise AuthenticationError()
        digest = hashlib.sha256(token.encode()).hexdigest()
        async with self.authority.read() as view:
            raw = await view.connection.scalar(
                sa.select(credentials.c.record).where(credentials.c.token_hash == digest)
            )
        if raw is None:
            raise AuthenticationError()
        record = CredentialRecord.model_validate(raw)
        if record.revoked_at is not None:
            raise AuthenticationError()
        return Principal(
            record.caller_id,
            record.namespace_ids,
            record.subject_ids,
            record.source_ids,
            frozenset(record.capabilities),
            record.id,
        )

    @staticmethod
    def _context(actor: Principal) -> CallContext:
        return CallContext(
            caller_id=actor.caller_id,
            namespace_id="instance",
            credential_source_ids=actor.source_ids,
        )

    @staticmethod
    def whoami(actor: Principal) -> PrincipalRecord:
        return PrincipalRecord(
            caller_id=actor.caller_id,
            credential_id=actor.credential_id,
            namespace_ids=actor.namespace_ids,
            subject_ids=actor.subject_ids,
            source_ids=actor.source_ids,
            capabilities=tuple(sorted(actor.capabilities)),
        )

    @staticmethod
    async def _record_in(
        connection: AsyncConnection, actor: Principal, credential_id: str
    ) -> CredentialRecord:
        raw = await connection.scalar(
            sa.select(credentials.c.record).where(credentials.c.id == credential_id)
        )
        if raw is None:
            raise DomainError("credential_not_found", "Credential does not exist")
        record = CredentialRecord.model_validate(raw)
        if not actor.permits_record(record):
            raise DomainError(
                "credential_scope_forbidden", "Credential is outside the manager's scope"
            )
        return record

    @staticmethod
    async def _event_in(
        tx: WriteTx,
        actor: Principal,
        record: CredentialRecord,
        action: Literal["created", "rotated", "revoked"],
        reason: str,
    ) -> None:
        event = CredentialEvent(
            id=str(uuid4()),
            credential_id=record.id,
            action=action,
            revision=record.revision,
            caller_id=actor.caller_id,
            recorded_at=datetime.fromisoformat(tx.recorded_at),
            commit_seq=tx.commit_seq,
            reason=reason,
            record=record,
        )
        await tx.connection.execute(
            credential_events.insert().values(
                id=event.id,
                credential_id=event.credential_id,
                revision=event.revision,
                commit_seq=event.commit_seq,
                record=event.model_dump(mode="json"),
            )
        )

    @staticmethod
    def _revision(record: CredentialRecord, expected: int | None) -> None:
        if expected is not None and expected != record.revision:
            raise DomainError(
                "credential_revision_conflict",
                "Credential revision changed; read the current record before retrying",
                expected_revision=expected,
                actual_revision=record.revision,
            )

    async def read(self, actor: Principal, credential_id: str) -> CredentialRecord:
        actor.require("manage")
        async with self.authority.read(self._context(actor)) as view:
            return await self._record_in(view.connection, actor, credential_id)

    async def history(self, actor: Principal, credential_id: str) -> tuple[CredentialEvent, ...]:
        actor.require("manage")
        async with self.authority.read(self._context(actor)) as view:
            await self._record_in(view.connection, actor, credential_id)
            rows = (
                (
                    await view.connection.execute(
                        sa.select(credential_events.c.record)
                        .where(credential_events.c.credential_id == credential_id)
                        .order_by(credential_events.c.commit_seq)
                    )
                )
                .scalars()
                .all()
            )
        return tuple(CredentialEvent.model_validate(row) for row in rows)

    async def create(self, actor: Principal, command: CreateCredential) -> IssuedCredential:
        actor.require("manage")
        if not command.name.strip() or not command.capabilities:
            raise ValueError("Credential name and at least one capability are required")
        if not set(command.capabilities).issubset(actor.capabilities):
            raise DomainError(
                "credential_scope_forbidden",
                "Credential capabilities exceed the issuer's authority",
            )
        scopes: dict[str, tuple[str, ...] | None] = {}
        for field in ("namespace_ids", "subject_ids", "source_ids"):
            requested = (
                getattr(command, field)
                if field in command.model_fields_set
                else getattr(actor, field)
            )
            if requested is not None:
                requested = tuple(dict.fromkeys(requested))
                if any(not value for value in requested):
                    raise ValueError("Credential scope identities must not be empty")
            if not _scope_subset(requested, getattr(actor, field)):
                raise DomainError(
                    "credential_scope_forbidden", "Credential scope exceeds the issuer's authority"
                )
            scopes[field] = requested
        payload = {
            "name": command.name,
            **scopes,
            "capabilities": sorted(set(command.capabilities)),
            **({"reason": command.reason} if command.reason else {}),
        }
        async with self.authority.write(
            self._context(actor),
            command.operation_id,
            operation_kind="credentials.create",
            scope="credentials",
            payload=payload,
        ) as tx:
            if tx.replayed:
                record = CredentialRecord.model_validate(tx.replay_result)
                current = await tx.connection.scalar(
                    sa.select(credentials.c.record).where(credentials.c.id == record.id)
                )
                return IssuedCredential(record=CredentialRecord.model_validate(current), token=None)
            credential_id = str(uuid4())
            token = f"nous_{credential_id.replace('-', '')}_{secrets.token_urlsafe(32)}"
            record = CredentialRecord(
                id=credential_id,
                caller_id=f"api-key:{credential_id}",
                name=command.name,
                namespace_ids=scopes["namespace_ids"],
                subject_ids=scopes["subject_ids"],
                source_ids=scopes["source_ids"],
                capabilities=tuple(sorted(set(command.capabilities))),
                created_by=actor.caller_id,
                created_at=tx.recorded_at,
                commit_seq=tx.commit_seq,
            )
            await tx.connection.execute(
                credentials.insert().values(
                    id=record.id,
                    token_hash=hashlib.sha256(token.encode()).hexdigest(),
                    record=record.model_dump(mode="json"),
                )
            )
            await self._event_in(tx, actor, record, "created", command.reason)
            tx.result(record)
            return IssuedCredential(record=record, token=token)

    async def list(
        self, actor: Principal, *, include_revoked: bool = True
    ) -> tuple[CredentialRecord, ...]:
        actor.require("manage")
        async with self.authority.read(self._context(actor)) as view:
            rows = (await view.connection.execute(sa.select(credentials.c.record))).scalars().all()
        records = (CredentialRecord.model_validate(raw) for raw in rows)
        return tuple(
            sorted(
                (
                    record
                    for record in records
                    if actor.permits_record(record)
                    and (include_revoked or record.revoked_at is None)
                ),
                key=lambda record: record.commit_seq,
                reverse=True,
            )
        )

    async def revoke(
        self,
        actor: Principal,
        credential_id: str,
        operation_id: str,
        *,
        expected_revision: int | None = None,
        reason: str = "",
    ) -> CredentialRecord:
        actor.require("manage")
        if (expected_revision is not None and expected_revision < 1) or len(reason) > 4096:
            raise ValueError("Invalid credential revision or reason")
        async with self.authority.write(
            self._context(actor),
            operation_id,
            operation_kind="credentials.revoke",
            scope="credentials",
            payload={
                "credential_id": credential_id,
                **(
                    {"expected_revision": expected_revision}
                    if expected_revision is not None
                    else {}
                ),
                **({"reason": reason} if reason else {}),
            },
        ) as tx:
            if tx.replayed:
                return CredentialRecord.model_validate(tx.replay_result)
            record = await self._record_in(tx.connection, actor, credential_id)
            self._revision(record, expected_revision)
            if record.revoked_at is None:
                record = record.model_copy(
                    update={
                        "revoked_at": tx.recorded_at,
                        "commit_seq": tx.commit_seq,
                        "revision": record.revision + 1,
                    }
                )
                await tx.connection.execute(
                    credentials.update()
                    .where(credentials.c.id == record.id)
                    .values(record=record.model_dump(mode="json"))
                )
                await self._event_in(tx, actor, record, "revoked", reason)
            return tx.result(record)

    async def rotate(
        self, actor: Principal, credential_id: str, command: RotateCredential
    ) -> IssuedCredential:
        actor.require("manage")
        async with self.authority.write(
            self._context(actor),
            command.operation_id,
            operation_kind="credentials.rotate",
            scope="credentials",
            payload={"credential_id": credential_id, "command": command},
        ) as tx:
            if tx.replayed:
                original = CredentialRecord.model_validate(tx.replay_result)
                current = await self._record_in(tx.connection, actor, original.id)
                return IssuedCredential(record=current, token=None)
            record = await self._record_in(tx.connection, actor, credential_id)
            if not set(record.capabilities).issubset(actor.capabilities):
                raise DomainError(
                    "credential_scope_forbidden",
                    "Rotating this credential would exceed the manager's capabilities",
                )
            self._revision(record, command.expected_revision)
            if record.revoked_at is not None:
                raise DomainError(
                    "credential_revoked_conflict", "A revoked credential cannot be rotated"
                )
            token = f"nous_{record.id.replace('-', '')}_{secrets.token_urlsafe(32)}"
            record = record.model_copy(
                update={"revision": record.revision + 1, "commit_seq": tx.commit_seq}
            )
            await tx.connection.execute(
                credentials.update()
                .where(credentials.c.id == record.id)
                .values(
                    token_hash=hashlib.sha256(token.encode()).hexdigest(),
                    record=record.model_dump(mode="json"),
                )
            )
            await self._event_in(tx, actor, record, "rotated", command.reason)
            tx.result(record)
            return IssuedCredential(record=record, token=token)
