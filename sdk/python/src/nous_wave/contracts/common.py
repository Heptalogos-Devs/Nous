"""Transport independent identities and time coordinates."""

from __future__ import annotations

from datetime import UTC, datetime
from typing import Literal
from uuid import uuid4

from pydantic import BaseModel, ConfigDict, Field, JsonValue, model_validator


class Contract(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class TimePoint(Contract):
    domain: str = "unix_ms"
    coordinate: int
    precision: Literal["instant", "day", "month", "year", "tick"] = "instant"
    original: str | None = None


class TimeRange(Contract):
    domain: str = "unix_ms"
    start: int | None = None
    end: int | None = None

    @model_validator(mode="after")
    def ordered(self) -> TimeRange:
        if self.start is not None and self.end is not None and self.start > self.end:
            raise ValueError("time range starts after its end")
        return self

    def contains(self, point: TimePoint) -> bool:
        return (
            point.domain == self.domain
            and (self.start is None or self.start <= point.coordinate)
            and (self.end is None or point.coordinate < self.end)
        )


def utc_now() -> TimePoint:
    return TimePoint(coordinate=int(datetime.now(UTC).timestamp() * 1000))


class CognitiveTimes(Contract):
    event: TimePoint | None = None
    observation: TimePoint | None = None
    formation: TimePoint
    validity: TimeRange | None = None
    recorded: TimePoint


class CallContext(Contract):
    """Trusted context constructed by the authenticated transport or worker."""

    caller_id: str
    namespace_id: str
    subject_id: str | None = None
    request_id: str = Field(default_factory=lambda: str(uuid4()))
    locale: str = "en"
    clock: TimePoint = Field(default_factory=utc_now)
    allowed_source_ids: tuple[str, ...] | None = None

    def require_subject(self) -> str:
        if self.subject_id is None:
            raise DomainError("subject_required", "This operation requires a subject")
        return self.subject_id


class Operation(Contract):
    operation_id: str = Field(default_factory=lambda: str(uuid4()))


class DomainError(Exception):
    def __init__(self, code: str, message: str, **details: JsonValue) -> None:
        super().__init__(message)
        self.code = code
        self.details = details
