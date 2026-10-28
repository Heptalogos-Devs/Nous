from typing import Literal

from pydantic import Field, JsonValue

from .cognition import CognitiveKind, CognitiveRecord
from .common import Contract, TimeRange
from .media import MediaRecord, MediaReference


class TimeFilter(Contract):
    axis: Literal["event", "observation", "formation", "validity", "recorded"]
    range: TimeRange


class QueryRequest(Contract):
    text: str = ""
    mode: Literal["lexical", "dense", "hybrid", "vcp"] = "lexical"
    kinds: tuple[CognitiveKind, ...] = ()
    domains: tuple[Literal["cognition", "media"], ...] = ("cognition",)
    media: tuple[MediaReference, ...] = ()
    parts: tuple[str | MediaReference, ...] = ()
    limit: int = Field(default=10, ge=1, le=1000)
    reference: str | None = None
    history: bool = False
    as_of_commit: int | None = Field(default=None, ge=0)
    min_commit_seq: int | None = Field(default=None, ge=0)
    times: tuple[TimeFilter, ...] = ()
    source_ids: tuple[str, ...] | None = None
    source_type: str | None = None
    include_deleted: bool = False
    include_inactive: bool = False


class QueryHit(Contract):
    object: CognitiveRecord | MediaRecord
    score: float
    source_roots: tuple[str, ...]


class QueryCoverage(Contract):
    indexed_through: int
    authority_commit_seq: int
    complete: bool
    generation: str


class QueryResult(Contract):
    hits: tuple[QueryHit, ...]
    coverage: QueryCoverage
    plan: dict[str, JsonValue] = Field(default_factory=dict)
