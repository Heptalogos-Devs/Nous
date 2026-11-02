"""Transport independent identities and time coordinates."""

from __future__ import annotations

from datetime import UTC, datetime
from typing import Annotated, Literal
from uuid import uuid4

from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    SerializerFunctionWrapHandler,
    model_serializer,
    model_validator,
)

ObservationKind = Literal["import", "contact", "external_snapshot"]
FormationPurpose = Literal["memory", "graph", "memory_and_graph"]


class Contract(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class TimePoint(Contract):
    domain: str = "unix_ms"
    coordinate: int
    precision: Literal["instant", "day", "month", "year", "tick"] = "instant"
    original: str | None = None


class ModelProvenance(Contract):
    model_id: str = Field(min_length=1)
    task: str = Field(min_length=1)
    prompt_digest: Annotated[str, Field(pattern=r"^[0-9a-f]{64}$")]
    schema_digest: Annotated[str, Field(pattern=r"^[0-9a-f]{64}$")]
    input_digest: Annotated[str, Field(pattern=r"^[0-9a-f]{64}$")]
    output_digest: Annotated[str, Field(pattern=r"^[0-9a-f]{64}$")]
    generated_at: TimePoint


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


class ApplicableInput(Contract):
    """An omitted applicability permits inference; explicit null records unknown applicability."""

    validity: TimeRange | None = None

    @model_serializer(mode="wrap")
    def preserve_explicit_validity(
        self, handler: SerializerFunctionWrapHandler
    ) -> dict[str, object]:
        value: dict[str, object] = handler(self)
        if "validity" not in self.model_fields_set:
            value.pop("validity", None)
        return value


def utc_now() -> TimePoint:
    return TimePoint(coordinate=int(datetime.now(UTC).timestamp() * 1000))


class CognitiveTimes(Contract):
    event: TimePoint | None = None
    observation: TimePoint | None = None
    formation: TimePoint
    validity: TimeRange | None = None
    recorded: TimePoint


class Operation(Contract):
    """Automatically assigned request identity; transaction commits own persistence."""

    operation_id: str = Field(default_factory=lambda: str(uuid4()))


class ReferenceAddresses(Contract):
    object: str | None = None
    exact: str
