"""Scoped instance API credentials; raw tokens are returned only at creation."""

from typing import Literal

from pydantic import Field

from .common import Contract, Operation

Capability = Literal["read", "write", "manage"]


class CreateCredential(Operation):
    name: str = Field(min_length=1, max_length=120)
    namespace_ids: tuple[str, ...] | None = None
    subject_ids: tuple[str, ...] | None = None
    source_ids: tuple[str, ...] | None = None
    capabilities: tuple[Capability, ...] = ("read",)


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


class IssuedCredential(Contract):
    record: CredentialRecord
    token: str | None = Field(default=None, repr=False)
