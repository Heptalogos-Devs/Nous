"""Typed identity client operations."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from uuid import uuid4

from pydantic import JsonValue, TypeAdapter

from ..contracts.credentials import (
    CreateCredential,
    CredentialEvent,
    CredentialRecord,
    IssuedCredential,
    PrincipalRecord,
    RotateCredential,
)
from ..operations import encoded, object_value, segment
from ..types import (
    JsonObject,
    Namespace,
    Subject,
)
from .core import ClientCore
from .requests import operation_identity, operation_values, required_text


class IdentityClient(ClientCore):
    async def create_namespace(self, name: str, *, operation_id: str | None = None) -> Namespace:
        return Namespace.model_validate(
            await self.request(
                "POST",
                "/api/namespaces",
                body={"name": name, "operation_id": operation_id or str(uuid4())},
            )
        )

    async def namespaces(self) -> tuple[Namespace, ...]:
        return TypeAdapter(tuple[Namespace, ...]).validate_python(
            await self.request("GET", "/api/namespaces")
        )

    async def create_subject(
        self,
        namespace_id: str,
        *,
        name: str = "default",
        capabilities: Sequence[str] = ("memory",),
        operation_id: str | None = None,
    ) -> Subject:
        return Subject.model_validate(
            await self.request(
                "POST",
                f"/api/namespaces/{segment(namespace_id)}/subjects",
                body={
                    "name": name,
                    "capabilities": list(capabilities),
                    "operation_id": operation_id or str(uuid4()),
                },
            )
        )

    async def subjects(self, namespace_id: str) -> tuple[Subject, ...]:
        return TypeAdapter(tuple[Subject, ...]).validate_python(
            await self.request("GET", f"/api/namespaces/{segment(namespace_id)}/subjects")
        )

    async def subject(self, subject_id: str) -> Subject:
        return Subject.model_validate(
            await self.request("GET", f"/api/subjects/{segment(subject_id)}")
        )

    async def subject_diagnostics(self, subject_id: str) -> JsonObject:
        return object_value(
            await self.request("GET", f"/api/subjects/{segment(subject_id)}/diagnostics")
        )

    async def whoami(self) -> PrincipalRecord:
        return PrincipalRecord.model_validate(await self.request("GET", "/api/credentials/whoami"))

    async def create_credential(self, command: CreateCredential) -> IssuedCredential:
        return IssuedCredential.model_validate(
            await self.request("POST", "/api/credentials", body=encoded(command))
        )

    async def credentials(self, *, include_revoked: bool = True) -> tuple[CredentialRecord, ...]:
        return TypeAdapter(tuple[CredentialRecord, ...]).validate_python(
            await self.request(
                "GET", "/api/credentials", params={"include_revoked": include_revoked}
            )
        )

    async def credential(self, credential_id: str) -> CredentialRecord:
        return CredentialRecord.model_validate(
            await self.request("GET", f"/api/credentials/{segment(credential_id)}")
        )

    async def credential_history(self, credential_id: str) -> tuple[CredentialEvent, ...]:
        return TypeAdapter(tuple[CredentialEvent, ...]).validate_python(
            await self.request("GET", f"/api/credentials/{segment(credential_id)}/history")
        )

    async def rotate_credential(
        self, credential_id: str, command: RotateCredential
    ) -> IssuedCredential:
        return IssuedCredential.model_validate(
            await self.request(
                "POST", f"/api/credentials/{segment(credential_id)}/rotate", body=encoded(command)
            )
        )

    async def revoke_credential(
        self,
        credential_id: str,
        *,
        expected_revision: int | None = None,
        reason: str = "",
        operation_id: str | None = None,
    ) -> CredentialRecord:
        return CredentialRecord.model_validate(
            await self.request(
                "DELETE",
                f"/api/credentials/{segment(credential_id)}",
                params={
                    "operation_id": operation_id or str(uuid4()),
                    "expected_revision": expected_revision,
                    "reason": reason,
                },
            )
        )

    async def identity(self, request: Mapping[str, JsonValue]) -> JsonValue:
        values = operation_values(request)
        action = request.get("action")
        if action == "whoami":
            return encoded(await self.whoami())
        if action == "list":
            rows = await self.credentials(
                include_revoked=TypeAdapter(bool).validate_python(
                    values.pop("include_revoked", True)
                )
            )
            after = values.pop("after", None)
            if after is not None:
                offset = next(
                    (index + 1 for index, row in enumerate(rows) if row.id == after), None
                )
                if offset is None:
                    raise ValueError("Credential cursor is unavailable")
                rows = rows[offset:]
            limit = TypeAdapter(int).validate_python(values.pop("limit", 100))
            if not 1 <= limit <= 1000:
                raise ValueError("limit must be between 1 and 1000")
            return [encoded(row) for row in rows[:limit]]
        if action == "issue":
            return encoded(await self.create_credential(CreateCredential.model_validate(values)))
        if action == "credential":
            raise ValueError(
                "Credential plaintext is delivered only when issuing or rotating the key"
            )
        credential_id = required_text(values, "id")
        if action == "read":
            return encoded(await self.credential(credential_id))
        if action == "history":
            return [encoded(row) for row in await self.credential_history(credential_id)]
        if action == "rotate":
            return encoded(
                await self.rotate_credential(credential_id, RotateCredential.model_validate(values))
            )
        if action == "revoke":
            return encoded(
                await self.revoke_credential(
                    credential_id,
                    expected_revision=TypeAdapter(int | None).validate_python(
                        values.pop("expected_revision", None)
                    ),
                    reason=TypeAdapter(str).validate_python(values.pop("reason", "")),
                    operation_id=operation_identity(values),
                )
            )
        if action == "credential":
            raise ValueError(
                "Credential plaintext is delivered only when issuing or rotating the key"
            )
        raise ValueError("Unknown identity action")
