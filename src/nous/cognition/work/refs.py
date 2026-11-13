"""Lightweight identities for a work and its exact revisions."""

from typing import Literal

from pydantic import Field

from nous.cognition.contracts import CognitiveValue


class WorkRef(CognitiveValue):
    kind: Literal["work"] = "work"
    instance_id: str
    namespace_id: str
    id: str
    identifier: str


class WorkRevisionRef(CognitiveValue):
    kind: Literal["work_revision"] = "work_revision"
    work_ref: WorkRef
    revision_id: str
    ordinal: int = Field(ge=1)

    @property
    def identifier(self) -> str:
        return f"{self.work_ref.identifier}@{self.ordinal}"
