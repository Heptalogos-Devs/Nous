"""A host reports a concrete encounter with an exact retained source."""

from pydantic import Field, JsonValue

from .cognition import CognitiveRef
from .common import Contract, Operation, TimePoint
from .imports import ObservationFormSubmission
from .media import MediaReference, MediaStrategy


class ContactMaterial(Operation):
    source: MediaReference
    external_event_id: str = Field(min_length=1, max_length=4096)
    observed_at: TimePoint | None = None
    event_time: TimePoint | None = None
    origin: dict[str, str] = Field(default_factory=dict)
    participants: tuple[str, ...] = ()
    audience: tuple[str, ...] = ()
    statement: str | None = Field(
        default=None, min_length=1, max_length=1000000, exclude_if=lambda value: value is None
    )
    form_long_term: bool = False
    media_strategy: MediaStrategy = "description"
    related: tuple[CognitiveRef, ...] = Field(
        default=(), max_length=1000, exclude_if=lambda values: not values
    )

    def identity_payload(self) -> dict[str, JsonValue]:
        excluded = {"operation_id"}
        if self.media_strategy == "description":
            excluded.add("media_strategy")
        return self.model_dump(mode="json", exclude=excluded)


class ContactReceipt(Contract):
    observation_id: str
    namespace_id: str
    caller_id: str
    external_event_id: str
    source: MediaReference
    active_sources: tuple[MediaReference, ...]
    cognition: tuple[CognitiveRef, ...] = ()
    formation: ObservationFormSubmission | None = None
    commit_seq: int
