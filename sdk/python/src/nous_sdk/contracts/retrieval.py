from typing import Literal

from pydantic import Field, JsonValue, model_validator

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
from .query_context import QueryContext
from .query_readout import RerankInfo
from .query_vcp import VcpIntent, VcpResult
from .resources import ResourceRecord
from .retrieval_policy import RetrievalPolicy
from .source_graph import ExpandedSources
from .source_objects import EvidenceRelationRecord, ObservationRecord

QueryObject = (
    CognitiveRecord | MediaRecord | ObservationRecord | EvidenceRelationRecord | ResourceRecord
)


class TimeFilter(Contract):
    axis: Literal["event", "observation", "formation", "validity", "recorded"]
    range: TimeRange


class QueryRequest(Contract):
    context: QueryContext | str | None = None
    background: str = Field(default="", max_length=8192)
    references: tuple[str, ...] = Field(default=(), max_length=64)
    wave_profile: str | None = None
    capability_policy: Literal["strict", "best_effort"] | None = None
    reranker: Literal["compute", "cross_encoder", "llm", "off"] | None = None
    profile: RetrievalPolicy | None = None
    vcp: VcpIntent = Field(default_factory=VcpIntent)
    text: str = ""
    expression: QueryExpression | None = None
    filters: QueryFilters = Field(default_factory=QueryFilters)
    budget: QueryBudget = Field(default_factory=QueryBudget)
    capabilities: QueryCapabilities = Field(default_factory=QueryCapabilities)
    current: Literal["none", "prefer", "required"] = "none"
    rerank: Literal["off", "optional", "required"] = "off"
    materialize: bool = False
    mode: Literal["lexical", "dense", "hybrid", "vcp"] = "lexical"
    kinds: tuple[CognitiveKind, ...] = ()
    domains: tuple[Literal["cognition", "media", "observation", "evidence", "resource"], ...] = (
        "cognition",
        "media",
    )

    @model_validator(mode="before")
    @classmethod
    def typed_query_domain(cls, value):
        if isinstance(value, dict) and value.get("kinds") and "domains" not in value:
            return {**value, "domains": ("cognition",)}
        return value

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
    object: QueryObject
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
    result_refs: tuple[str, ...] = ()

    @property
    def text(self) -> str:
        return self.view().text

    def view(self):
        from nous_sdk.presentation import present_query

        return present_query(self)

    vcp: VcpResult | None = None
    hits: tuple[QueryHit, ...]
    coverage: QueryCoverage
    execution: QueryExecution | None = None
    expression: str = ""
    sources: ExpandedSources | None = None
    rerank: RerankInfo | None = None
    plan: dict[str, JsonValue] = Field(default_factory=dict)
