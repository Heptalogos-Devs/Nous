"""The bounded working situation supplied to a query."""

from pydantic import Field

from .common import Contract, TimePoint
from .media import MediaReference


class QueryContext(Contract):
    goal: str = Field(default="", max_length=8192)
    anchors: tuple[str, ...] = Field(default=(), max_length=64)
    entities: tuple[str, ...] = Field(default=(), max_length=32)
    attachments: tuple[MediaReference, ...] = Field(default=(), max_length=32)
    reference_time: TimePoint | None = None
    unresolved: tuple[str, ...] = Field(default=(), max_length=64)
