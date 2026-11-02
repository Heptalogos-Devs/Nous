"""Namespace identity, stable host bindings and instance credentials."""
from __future__ import annotations
from collections.abc import Mapping
from uuid import uuid4
from pydantic import JsonValue, TypeAdapter
from ..contracts.credentials import CreateCredential, CredentialEvent, CredentialRecord, IssuedCredential, PrincipalRecord, RotateCredential
from ..contracts.diagnostics import NamespaceDiagnostics
from ..contracts.identity import BindSubject, BoundSubject, ChangeNamespace, IdentityCapabilities, IdentityDirectoryQuery, IdentityPurpose, Namespace, NamespacePage, NamespacePurgeResult, PurgeNamespace, ReadSubjectBinding
from ..contracts.namespace_archives import NamespaceArchive, NamespaceRestoreResult, RestoreNamespace
from ..operations import encoded, segment
from .backups import NamespaceBackupClient
from .requests import operation_identity, operation_values, required_text

class IdentityClient(NamespaceBackupClient):
    async def create_namespace(self, slug: str, *, namespace_id: str | None = None, display_name: str | None = None, mode: str = 'knowledge', subject_key: str | None = None, enabled_features: tuple[str,...] | None = None, operation_id: str | None = None) -> Namespace:
        from ..contracts.identity import CreateNamespace
        command = CreateNamespace(slug=slug, id=namespace_id, display_name=display_name, mode=mode, subject_key=subject_key, enabled_features=enabled_features, operation_id=operation_id or str(uuid4()))
        return Namespace.model_validate(await self.request('POST','/api/v1/namespaces',body=encoded(command)))


    async def get_namespace(self, namespace_id: str, *, purpose: IdentityPurpose = 'active') -> Namespace:
        return Namespace.model_validate(await self.request('GET',f'/api/v1/namespaces/{segment(namespace_id)}',params={'purpose':purpose}))

    async def namespace_page(self, command: IdentityDirectoryQuery | None = None) -> NamespacePage:
        return NamespacePage.model_validate(await self.request('POST','/api/v1/namespaces/query',body=encoded(command or IdentityDirectoryQuery())))

    async def change_namespace(self, namespace_id: str, command: ChangeNamespace) -> Namespace:
        return Namespace.model_validate(await self.request('POST',f'/api/v1/namespaces/{segment(namespace_id)}/change',body=encoded(command)))

    async def identity_capabilities(self, namespace_id: str, *, purpose: IdentityPurpose = 'active') -> IdentityCapabilities:
        return IdentityCapabilities.model_validate(await self.request('GET',f'/api/v1/namespaces/{segment(namespace_id)}/capabilities',params={'purpose':purpose}))

    async def bind_subject(self, namespace_id: str, command: BindSubject) -> BoundSubject:
        return BoundSubject.model_validate(await self.request('POST',f'/api/v1/namespaces/{segment(namespace_id)}/subject-bindings',body=encoded(command)))

    async def subject_binding(self, namespace_id: str, external_id: str, *, purpose: IdentityPurpose = 'active') -> BoundSubject:
        return BoundSubject.model_validate(await self.request('POST',f'/api/v1/namespaces/{segment(namespace_id)}/subject-bindings/read',body=encoded(ReadSubjectBinding(external_id=external_id,purpose=purpose))))

    async def namespace_diagnostics(self, namespace_id: str) -> NamespaceDiagnostics:
        return NamespaceDiagnostics.model_validate(await self.request('GET',f'/api/v1/namespaces/{segment(namespace_id)}/diagnostics'))

    async def purge_namespace(self, namespace_id: str, command: PurgeNamespace) -> NamespacePurgeResult:
        return NamespacePurgeResult.model_validate(await self.request('POST',f'/api/v1/namespaces/{segment(namespace_id)}/purge',body=encoded(command)))

    async def export_namespace_archive(self, namespace_id: str) -> NamespaceArchive:
        return NamespaceArchive.model_validate(await self.request('POST',f'/api/v1/namespaces/{segment(namespace_id)}/archive'))

    async def restore_namespace_archive(self, namespace_id: str, command: RestoreNamespace) -> NamespaceRestoreResult:
        return NamespaceRestoreResult.model_validate(await self.request('POST',f'/api/v1/namespaces/{segment(namespace_id)}/restore-archive',body=encoded(command)))

    async def whoami(self) -> PrincipalRecord:
        return PrincipalRecord.model_validate(await self.request("GET", "/api/v1/credentials/whoami"))

    async def create_credential(self, command: CreateCredential) -> IssuedCredential:
        return IssuedCredential.model_validate(
            await self.request("POST", "/api/v1/credentials", body=encoded(command))
        )

    async def credentials(self, *, include_revoked: bool = True) -> tuple[CredentialRecord, ...]:
        return TypeAdapter(tuple[CredentialRecord, ...]).validate_python(
            await self.request(
                "GET", "/api/v1/credentials", params={"include_revoked": include_revoked}
            )
        )

    async def credential(self, credential_id: str) -> CredentialRecord:
        return CredentialRecord.model_validate(
            await self.request("GET", f"/api/v1/credentials/{segment(credential_id)}")
        )

    async def credential_history(self, credential_id: str) -> tuple[CredentialEvent, ...]:
        return TypeAdapter(tuple[CredentialEvent, ...]).validate_python(
            await self.request("GET", f"/api/v1/credentials/{segment(credential_id)}/history")
        )

    async def rotate_credential(
        self, credential_id: str, command: RotateCredential
    ) -> IssuedCredential:
        return IssuedCredential.model_validate(
            await self.request(
                "POST", f"/api/v1/credentials/{segment(credential_id)}/rotate", body=encoded(command)
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
                f"/api/v1/credentials/{segment(credential_id)}",
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
