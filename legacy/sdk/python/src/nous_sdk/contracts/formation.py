"""Small model outputs used by automatic note and graph formation."""

from typing import Literal

from pydantic import Field, JsonValue

from .common import Contract


class MemoryNote(Contract):
    text: str = Field(min_length=1)
    topics: tuple[str, ...] = ()
    source_keys: tuple[str, ...] = Field(min_length=1)
    time_expression: str | None = None
    previous_key: str | None = None
    relation: Literal["new", "same", "supplement", "changed", "conflict"]


class NotesDraft(Contract):
    notes: tuple[MemoryNote, ...] = Field(default=(), max_length=128)


class GraphEntity(Contract):
    name: str = Field(min_length=1)
    entity_type: Literal["person", "group", "organization", "place", "object", "other"] = "other"
    aliases: tuple[str, ...] = ()
    description: str = ""


class GraphRelation(Contract):
    subject: str = Field(min_length=1)
    relation: str = Field(min_length=1)
    object: str = Field(min_length=1)
    explanation: str = ""


class GraphDraft(Contract):
    entities: tuple[GraphEntity, ...] = Field(default=(), max_length=256)
    relations: tuple[GraphRelation, ...] = Field(default=(), max_length=256)


class GraphResolutionInput(Contract):
    source_text: str
    resolution_data: JsonValue


class EntityResolution(Contract):
    entity_key: str = Field(min_length=1)
    candidate_key: str | None = None


class GraphResolutionDraft(Contract):
    entities: tuple[EntityResolution, ...] = Field(min_length=1, max_length=256)
