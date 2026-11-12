"""Types shared by complete application operations and their callers."""

from __future__ import annotations

import asyncio
import inspect
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from typing import TYPE_CHECKING, Literal

from pydantic import BaseModel, ConfigDict, TypeAdapter
from typing_extensions import TypeForm

from nous.application.product_refs import ProductRef
from nous.common.operations import OperationIssue, record_completed

if TYPE_CHECKING:
    from nous.application.inputs import InputReader
    from nous.configuration import ConfigSnapshot
    from nous.runtime.execution import OperationContext


class Event(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")


class Progress(Event):
    kind: Literal["progress"] = "progress"
    stage: str
    completed: int | None = None
    total: int | None = None


class TextDelta(Event):
    kind: Literal["text_delta"] = "text_delta"
    channel: str
    text: str


class Saved(Event):
    kind: Literal["saved"] = "saved"
    refs: tuple[ProductRef, ...]


class Notice(Event):
    kind: Literal["notice"] = "notice"
    issue: OperationIssue


type BasicEvents = Progress | Saved | Notice
type LanguageEvents = Progress | TextDelta | Saved | Notice
type OperationEvent = LanguageEvents


class EventEnvelope(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")

    operation_id: str
    sequence: int
    event: OperationEvent


type EventObserver = Callable[[EventEnvelope], Awaitable[None] | None]


class EventDeliveryError(Exception):
    """The foreground observer could not receive an operation event."""


class EventEmitter:
    """Deliver events in order, applying backpressure to the producing operation."""

    def __init__(
        self,
        operation_id: str,
        adapter: TypeAdapter[OperationEvent],
        observer: EventObserver | None = None,
    ) -> None:
        self.operation_id = operation_id
        self._adapter = adapter
        self._observer = observer
        self._sequence = 0
        self._lock = asyncio.Lock()

    async def emit(self, event: OperationEvent) -> None:
        checked = self._adapter.validate_python(event)
        if self._observer is None:
            return
        async with self._lock:
            self._sequence += 1
            try:
                outcome = self._observer(
                    EventEnvelope(
                        operation_id=self.operation_id, sequence=self._sequence, event=checked
                    )
                )
                if inspect.isawaitable(outcome):
                    await outcome
            except Exception as error:
                raise EventDeliveryError("The operation observer failed") from error


@dataclass(frozen=True)
class CallContext:
    """The current call's execution, external inputs, and event delivery."""

    execution: OperationContext
    inputs: InputReader
    events: EventEmitter

    @property
    def settings(self) -> ConfigSnapshot:
        return self.execution.settings

    @property
    def deadline(self) -> float | None:
        return self.execution.deadline

    @property
    def operation_id(self) -> str:
        return self.execution.operation_id

    @property
    def stage(self) -> str:
        return self.execution.stage

    async def saved(self, *refs: ProductRef) -> None:
        """Announce references only after the owning transaction has committed."""
        record_completed(refs)
        await self.events.emit(Saved(refs=refs))


@dataclass(frozen=True)
class OperationDefinition[Q, R]:
    name: str
    request_adapter: TypeAdapter[Q]
    result_adapter: TypeAdapter[R]
    event_adapter: TypeAdapter[OperationEvent]
    description: str = ""


def define_operation[Q, R](
    name: str,
    *,
    request: TypeForm[Q],
    result: TypeForm[R],
    events: TypeForm[OperationEvent] = BasicEvents,
    description: str = "",
) -> OperationDefinition[Q, R]:
    """Build the serializers and schemas once, at the capability's definition."""
    return OperationDefinition(
        name=name,
        request_adapter=TypeAdapter(request),
        result_adapter=TypeAdapter(result),
        event_adapter=TypeAdapter(events),
        description=description,
    )


@dataclass(frozen=True)
class BoundOperation[Q, R]:
    definition: OperationDefinition[Q, R]
    handle: Callable[[Q, CallContext], Awaitable[R]]
