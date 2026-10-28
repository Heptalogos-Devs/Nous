"""Explicit source selectors shared by scoped SDK calls and HTTP admission."""

from typing import Annotated, Literal

from pydantic import Field, model_validator

from .contracts import CognitiveRef, Contract, MediaReference


class OriginalMedia(Contract):
    kind: Literal["original"] = "original"
    material_id: str


class CognitionSource(Contract):
    kind: Literal["cognition"] = "cognition"
    reference: CognitiveRef


class MediaSource(Contract):
    kind: Literal["media"] = "media"
    reference: MediaReference | OriginalMedia
    observation_id: str | None = None


class ObservationSource(Contract):
    kind: Literal["observation"] = "observation"
    id: str


class WorkContextSource(Contract):
    kind: Literal["work_context"] = "work_context"
    id: str


SourceReadRef = Annotated[
    CognitionSource | MediaSource | ObservationSource | WorkContextSource,
    Field(discriminator="kind"),
]


class SourceReadScope(Contract):
    subject_id: str = Field(min_length=1)
    source_refs: tuple[SourceReadRef, ...] = Field(default=(), max_length=1024)
    origin_conversation_refs: tuple[str, ...] = Field(default=(), max_length=64)
    observation_ids: tuple[str, ...] = Field(default=(), max_length=1024)

    @model_validator(mode="after")
    def distinct(self) -> "SourceReadScope":
        encoded = [ref.model_dump_json() for ref in self.source_refs]
        if (
            len(set(encoded)) != len(encoded)
            or len(set(self.origin_conversation_refs)) != len(self.origin_conversation_refs)
            or len(set(self.observation_ids)) != len(self.observation_ids)
        ):
            raise ValueError("source scope selectors must be distinct")
        if any(
            not value.strip() for value in (*self.origin_conversation_refs, *self.observation_ids)
        ):
            raise ValueError("source identities must be nonempty")
        return self
