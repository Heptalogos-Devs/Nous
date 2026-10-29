"""Scoped instance API credentials; raw tokens are returned only at creation."""

from typing import Literal

from pydantic import AwareDatetime, Field

from .common import Contract, Operation

Capability = Literal["read", "write", "manage"]


class CreateCredential(Operation):
    name: str = Field(min_length=1, max_length=120)
    namespace_ids: tuple[str, ...] | None = None
    subject_ids: tuple[str, ...] | None = None
    source_ids: tuple[str, ...] | None = None
    capabilities: tuple[Capability, ...] = ("read",)
    reason: str = Field(default="", max_length=4096)


class RotateCredential(Operation):
    expected_revision: int = Field(ge=1)
    reason: str = Field(default="", max_length=4096)


class PrincipalRecord(Contract):
    caller_id: str
    credential_id: str | None = None
    namespace_ids: tuple[str, ...] | None = None
    subject_ids: tuple[str, ...] | None = None
    source_ids: tuple[str, ...] | None = None
    capabilities: tuple[Capability, ...]


class CredentialRecord(Contract):
    id: str
    caller_id: str
    name: str
    namespace_ids: tuple[str, ...] | None = None
    subject_ids: tuple[str, ...] | None = None
    source_ids: tuple[str, ...] | None = None
    capabilities: tuple[Capability, ...]
    created_by: str
    created_at: str
    revoked_at: str | None = None
    commit_seq: int
    revision: int = Field(default=1, ge=1)


class CredentialEvent(Contract):
    id: str
    credential_id: str
    action: Literal["created", "rotated", "revoked"]
    revision: int = Field(ge=1)
    caller_id: str
    recorded_at: AwareDatetime
    commit_seq: int
    reason: str = ""
    record: CredentialRecord


class IssuedCredential(Contract):
    record: CredentialRecord
    token: str | None = Field(default=None, repr=False)
