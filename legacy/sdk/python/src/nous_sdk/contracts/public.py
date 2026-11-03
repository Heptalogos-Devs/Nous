"""User-facing operations shared by the SDK and HTTP."""
from pydantic import Field, JsonValue
from .common import Contract, Operation
from .source_graph import ExpandedSources

class RememberInput(Operation):
    content: str = Field(min_length=1)
    source: str | None = None
    tags: tuple[str, ...] = ()
    pin: bool = False

class CorrectionInput(Operation):
    ref: str = Field(min_length=1)
    content: str = Field(min_length=1)
    reason: str = ''
    source: str | None = None

class ReadRequest(Contract):
    ref: str = Field(min_length=1)
    cursor: str | None = None
    sources: bool = False
    offset: int = Field(default=0,ge=0)
    max_chars: int = Field(default=12000,ge=1,le=1000000)

class ReadView(Contract):
    ref: str
    kind: str
    title: str
    text: str
    value: JsonValue
    sources: ExpandedSources | None = None
    next_cursor: str | None = None
