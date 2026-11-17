"""Immutable cognitive queries, actual preparation context and delivered selections."""

from typing import Literal

from pydantic import Field, JsonValue

from nous.content.contracts import ExternalInput
from nous.wave.contracts import (
    CandidateFilter,
    ContextFragment,
    PassageDelivery,
    QueryCue,
    RankEvidence,
    SearchCoverage,
    SelectedContext,
    SnapshotRef,
)

from ..contracts import (
    CognitiveValue,
    Namespace,
    NamespaceSelector,
    RecordRevisionRef,
    RevisionSelection,
    SourceBinding,
)

Mode = Literal["auto", "lexical", "dense", "tagmemo", "rivermemo", "hybrid", "dtsc"]
SearchMode = Literal["lexical", "dense", "tagmemo", "rivermemo", "hybrid", "dtsc"]


class CognitiveGroupContext(CognitiveValue):
    revision_ref: RecordRevisionRef
    kind: Literal["note", "journal"]
    purpose: str | None
    sources: tuple[SourceBinding, ...]
    needs_review: bool


class CognitiveQuery(CognitiveValue):
    text: str = ""
    namespace: NamespaceSelector = None
    mode: Mode | None = None
    snapshot: str | SnapshotRef | None = None
    top_k: int | None = Field(default=None, ge=1)
    include_review: bool | None = None
    include_exploration: bool | None = None
    explain: bool = False
    context: tuple[ContextFragment, ...] | ExternalInput = ()
    core_concepts: tuple[str, ...] = ()
    cues: tuple[QueryCue, ...] = ()
    filters: CandidateFilter | ExternalInput | None = None
    purpose: str | None = None


class RefreshRetrieval(CognitiveValue):
    namespace: NamespaceSelector = None
    mode: Mode | None = None
    include_review: bool | None = None
    include_exploration: bool | None = None


class RefreshResult(CognitiveValue):
    namespace: Namespace
    snapshot: SnapshotRef
    captured_generation: int | None
    current_generation: int
    profile: str | None
    metadata: dict[str, JsonValue]
    current: bool


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


class RevisionPassage(CognitiveValue):
    selection: RevisionSelection
    delivery: PassageDelivery


class CognitiveGroup(CognitiveValue):
    group_key: str
    context: CognitiveGroupContext
    title: str | None
    rank: int = Field(ge=1)
    basis: RankEvidence
    passages: tuple[RevisionPassage, ...]


class QueryOperationContext(CognitiveValue):
    operation_id: str
    root_name: str
    stage: str


class QueryResult(CognitiveValue):
    context: QueryContext
    namespace: Namespace
    current: bool
    groups: tuple[CognitiveGroup, ...]
    coverage: SearchCoverage
    operation_context: QueryOperationContext
    score_kind: str
    used_lanes: tuple[str, ...]
    skipped_lanes: tuple[dict[str, JsonValue], ...]
    algorithm_versions: dict[str, str]
    needs_review_count: int = 0
    thinking: dict[str, JsonValue] | None = None
    diagnostics: dict[str, JsonValue] | None = None
