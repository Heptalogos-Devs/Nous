"""A Namespace owns one continuous cognitive state and its host bindings."""
from typing import Literal
from pydantic import Field, model_validator, field_validator
from uuid import UUID
from .common import Contract, Operation
IdentityPurpose = Literal['active', 'management']
NamespaceMode = Literal['knowledge', 'subject']
NamespaceState = Literal['active', 'disabled', 'trashed']
SUPPORTED_FEATURES = ('memory', 'episodes', 'person_profiles', 'ego', 'social', 'motivation')

class CreateNamespace(Operation):
    slug: str = Field(min_length=1, max_length=256, pattern=r'^[\w.-]+$')
    display_name: str | None = Field(default=None, min_length=1, max_length=256)
    mode: NamespaceMode = 'knowledge'
    subject_key: str | None = Field(default=None, min_length=1, max_length=512)
    enabled_features: tuple[str, ...] | None = None
    id: str | None = None

    @field_validator('id')
    @classmethod
    def namespace_uuid(cls, value: str | None) -> str | None:
        return str(UUID(value)) if value is not None else None

class Namespace(Contract):
    id: str
    slug: str
    display_name: str
    mode: NamespaceMode = 'knowledge'
    subject_key: str | None = None
    enabled_features: tuple[str, ...] = ('memory','episodes','person_profiles')
    state: NamespaceState = 'active'
    revision: int = 1
    commit_seq: int = 0
    created_at: str | None = None
    updated_at: str | None = None
    purging: bool = False

    @field_validator('id')
    @classmethod
    def namespace_uuid(cls, value: str) -> str:
        return str(UUID(value))

    @model_validator(mode='after')
    def valid_features(self):
        unknown = set(self.enabled_features) - set(SUPPORTED_FEATURES)
        if unknown:
            raise ValueError(f'Unknown Namespace features: {sorted(unknown)}')
        if len(set(self.enabled_features)) != len(self.enabled_features):
            raise ValueError('Namespace features must be unique')
        return self


class IdentityDirectoryQuery(Contract):
    purpose: IdentityPurpose = 'active'
    limit: int = Field(default=100, ge=1, le=1000)
    cursor: str | None = None
    states: tuple[NamespaceState, ...] = ()
    subject_key: str | None = None

class NamespacePage(Contract):
    items: tuple[Namespace, ...]
    next_cursor: str | None = None

class ChangeNamespace(Operation):
    action: Literal['rename', 'configure_features', 'enable', 'disable', 'trash', 'restore']
    expected_revision: int | None = Field(default=None, ge=1)
    slug: str | None = Field(default=None, min_length=1, max_length=256, pattern=r'^[\w.-]+$')
    display_name: str | None = Field(default=None, min_length=1, max_length=256)
    enabled_features: tuple[str, ...] | None = None

class IdentityCapabilities(Contract):
    namespace_id: str
    supported: tuple[str, ...]
    configured: tuple[str, ...]
    enabled: tuple[str, ...]

class BindSubject(Operation):
    external_id: str = Field(min_length=1, max_length=512)
    host_id: str | None = Field(default=None, min_length=1, max_length=512)

class SubjectBinding(Contract):
    namespace_id: str
    host_id: str
    external_id: str
    commit_seq: int

class ReadSubjectBinding(Contract):
    external_id: str = Field(min_length=1, max_length=512)
    host_id: str | None = None
    purpose: IdentityPurpose = 'active'

class BoundSubject(Contract):
    binding: SubjectBinding
    namespace: Namespace

class PurgeNamespace(Operation):
    expected_revision: int | None = Field(default=None, ge=1)

class NamespacePurgeResult(Contract):
    namespace_id: str
    commit_seq: int
    deleted_rows: int
    released_hashes: tuple[str, ...] = ()
