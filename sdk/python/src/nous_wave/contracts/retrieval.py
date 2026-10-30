from typing import Literal

from pydantic import Field, JsonValue

from .cognition import CognitiveKind, CognitiveRecord
from .common import Contract, TimeRange
from .media import MediaRecord, MediaReference
from .query_conditions import (
    QueryAuthority,
    QueryBudget,
    QueryCapabilities,
    QueryExecution,
    QueryExpression,
    QueryFilters,
)
from .query_readout import RerankInfo
from .query_vcp import VcpIntent, VcpResult
from .source_graph import ExpandedSources


class TimeFilter(Contract):
    axis: Literal["event", "observation", "formation", "validity", "recorded"]
    range: TimeRange


class QueryRequest(Contract):
    vcp: VcpIntent = Field(default_factory=VcpIntent)
    text: str = ""
    expression: QueryExpression | None = None
    filters: QueryFilters = Field(default_factory=QueryFilters)
    budget: QueryBudget = Field(default_factory=QueryBudget)
    capabilities: QueryCapabilities = Field(default_factory=QueryCapabilities)
    current: Literal["none", "prefer", "required"] = "none"
    rerank: Literal["off", "optional", "required"] = "off"
    rerank_invocation_id: str | None = None
    materialize: bool = False
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
    rerank_score: float | None = None
    authority: QueryAuthority | None = None


class QueryCoverage(Contract):
    indexed_through: int
    authority_commit_seq: int
    complete: bool
    generation: str


class QueryResult(Contract):
    vcp: VcpResult | None = None
    hits: tuple[QueryHit, ...]
    coverage: QueryCoverage
    execution: QueryExecution | None = None
    expression: str = ""
    sources: ExpandedSources | None = None
    rerank: RerankInfo | None = None
    plan: dict[str, JsonValue] = Field(default_factory=dict)
