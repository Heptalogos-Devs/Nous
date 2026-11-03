"""Shared facts about original events and actual encounters, independent of wire roles."""

from typing import Literal
from uuid import uuid4

from pydantic import Field, JsonValue, model_validator

from .cognitive_identity import CognitiveRef
from .common import Contract, TimePoint
from .media import MediaReference


class EventActor(Contract):
    platform: str | None = None
    account_id: str | None = None
    name: str | None = None
    entity: CognitiveRef | None = None


class EventConversation(Contract):
    id: str
    platform: str | None = None
    thread_id: str | None = None
    name: str | None = None


class EventContext(Contract):
    author: EventActor | None = None
    observer: EventActor | None = None
    conversation: EventConversation | None = None
    participants: tuple[EventActor, ...] = ()
    audience: tuple[EventActor, ...] = ()
    occurred_at: TimePoint | None = None
    observed_at: TimePoint | None = None
    reply_to: str | None = None


class ContactEvent(Contract):
    event_id: str | None = None
    source: MediaReference | None = None
    payload: JsonValue = None
    text: str = ""
    context: EventContext = Field(default_factory=EventContext)
    role: Literal["system", "user", "assistant", "tool"] | None = None
    tool_call_id: str | None = None
    response_id: str | None = None

    @model_validator(mode="after")
    def actual_content(self):
        if self.source is None and self.payload is None and not self.text:
            raise ValueError("An event needs retained content or an exact source")
        return self


class EventBatch(Contract):
    events: tuple[ContactEvent, ...] = Field(min_length=1)
    delivery_id: str = Field(default_factory=lambda: str(uuid4()), min_length=1)
    thread_id: str | None = None
    work_id: str | None = None
    background: str = ""
    background_refs: tuple[MediaReference, ...] = ()
    closed: bool = True
    formation: Literal["automatic", "off"] = "automatic"
    flush: bool = False


class FormationScope(Contract):
    namespace_id: str
    observer: EventActor | None = None
    thread: str
    work_id: str | None = None


class PendingFormation(Contract):
    scope_id: str
    job_id: str
    closed_through: int


class AcceptedEvent(Contract):
    id: str
    external_event_id: str | None
    raw_source: MediaReference
    source: MediaReference
    observation_id: str
    context: EventContext
    commit_seq: int


class AcceptedContact(Contract):
    events: tuple[AcceptedEvent, ...]
    pending: tuple[PendingFormation, ...] = ()
    commit_seq: int

    @property
    def sources(self) -> tuple[MediaReference, ...]:
        return tuple(event.source for event in self.events)
