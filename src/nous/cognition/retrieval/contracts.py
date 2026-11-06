"""Immutable cognitive queries, actual preparation context and delivered selections."""

from typing import Literal

from pydantic import Field, JsonValue

from nous.content.contracts import ContentSelection
from nous.wave.contracts import (
    CandidateFilter,
    CandidateRef,
    ContextFragment,
    QueryCue,
    SearchCoverage,
    SelectedContext,
    SnapshotRef,
)

from ..contracts import (
    CognitiveValue,
    Namespace,
    RecordRevisionRef,
    RevisionSelection,
    SourceBinding,
)

Mode = Literal["auto", "lexical", "dense", "tagmemo", "rivermemo", "hybrid", "dtsc"]
SearchMode = Literal["lexical", "dense", "tagmemo", "rivermemo", "hybrid", "dtsc"]


class CognitiveQuery(CognitiveValue):
    text: str = ""
    namespace: str | None = None
    mode: Mode | None = None
    snapshot: str | None = None
    top_k: int | None = Field(default=None, ge=1)
    include_review: bool | None = None
    include_exploration: bool | None = None
    explain: bool = False
    context: tuple[ContextFragment, ...] = ()
    core_concepts: tuple[str, ...] = ()
    cues: tuple[QueryCue, ...] = ()
    filters: CandidateFilter | None = None
    purpose: str | None = None


class QueryContext(CognitiveValue):
    request_text: str
    selected_context: tuple[SelectedContext, ...]
    omitted_context_count: int = Field(ge=0)
    namespace_id: str
    snapshot_ref: SnapshotRef
    captured_generation: int | None
    profile_signature: str | None
    space_signature: str | None
    mode: SearchMode
    effective_query_options: dict[str, JsonValue]
    filters: CandidateFilter | None
    core_concept_refs: tuple[str, ...]
    temporary_cues: tuple[QueryCue, ...]
    purpose: str | None = None


class SearchPassage(CognitiveValue):
    candidate_ref: CandidateRef
    selection: RevisionSelection
    content: ContentSelection
    origin_contents: tuple[ContentSelection, ...] = Field(min_length=1)
    candidate_rank: int = Field(ge=1)
    text: str
    snippet: str
    quality_score: float | None
    ranking_score: float
    score_kind: str
    channels: tuple[str, ...]
    evidence_roots: tuple[str, ...]
    concept_refs: tuple[str, ...]
    channel_ranks: dict[str, int]
    channel_scores: dict[str, float]
    details: dict[str, JsonValue] = Field(default_factory=dict)


class SearchGroup(CognitiveValue):
    group_key: str
    revision_ref: RecordRevisionRef
    title: str | None
    group_rank: int = Field(ge=1)
    representative: CandidateRef
    passages: tuple[SearchPassage, ...] = Field(min_length=1)
    sources: tuple[SourceBinding, ...]
    needs_review: bool


class DeliveredSelection(CognitiveValue):
    revision_ref: RecordRevisionRef
    selection: RevisionSelection
    candidate_ref: CandidateRef
    content: ContentSelection
    group_rank: int
    candidate_rank: int


class QueryOperationContext(CognitiveValue):
    operation_id: str
    root_name: str
    stage: str


class QueryResult(CognitiveValue):
    context: QueryContext
    namespace: Namespace
    groups: tuple[SearchGroup, ...]
    delivery: tuple[DeliveredSelection, ...]
    coverage: SearchCoverage
    operation_context: QueryOperationContext
    score_kind: str
    used_lanes: tuple[str, ...]
    skipped_lanes: tuple[dict[str, JsonValue], ...]
    algorithm_versions: dict[str, str]
    needs_review_count: int = 0
    thinking: dict[str, JsonValue] | None = None
    diagnostics: dict[str, JsonValue] | None = None
