"""Stable identities of written records and long-lived work."""

from typing import Literal

from pydantic import BaseModel, ConfigDict, Field


class _Reference(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")


class RecordRef(_Reference):
    instance_id: str
    namespace_id: str
    kind: Literal["note", "journal"]
    id: str
    identifier: str


class RecordRevisionRef(_Reference):
    kind: Literal["record_revision"] = "record_revision"
    record_ref: RecordRef
    revision_id: str
    ordinal: int = Field(ge=1)

    @property
    def identifier(self) -> str:
        return f"{self.record_ref.identifier}@{self.ordinal}"


class WorkRef(_Reference):
    kind: Literal["work"] = "work"
    instance_id: str
    namespace_id: str
    id: str
    identifier: str


class WorkRevisionRef(_Reference):
    kind: Literal["work_revision"] = "work_revision"
    work_ref: WorkRef
    revision_id: str
    ordinal: int = Field(ge=1)

    @property
    def identifier(self) -> str:
        return f"{self.work_ref.identifier}@{self.ordinal}"
