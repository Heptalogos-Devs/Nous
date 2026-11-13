"""Immutable cognitive queries, actual preparation context and delivered selections."""

from datetime import datetime
from typing import Annotated, Literal

from pydantic import Field, JsonValue

from nous.cognition.concept_contracts import ConceptRef
from nous.cognition.material_contracts import (
    CognitiveMaterialView,
    MaterialAttributes,
    MaterialRef,
    MaterialSelection,
)
from nous.cognition.steward.contracts import ReadingBrief
from nous.cognition.work.refs import WorkRevisionRef
from nous.common.operations import OperationIssue
from nous.content.contracts import ContentSelection, ResultRef
from nous.content.selections import union_selections
from nous.wave.contracts import (
    CandidateFilter,
    CandidateRef,
    ContextFragment,
    DeliveredText,
    DeliveryBudget,
    DeliveryStatus,
    ProgramRunFacts,
    RankEvidence,
    ScoreValue,
    SearchCoverage,
    SnapshotRef,
    TimeCondition,
    WaveBranchCoverage,
)

from ..contracts import (
    CognitiveValue,
    Namespace,
    NamespaceRef,
    NamespaceSelector,
)


class CognitiveGroupContext(CognitiveValue):
    material: MaterialRef
    kind: str
    purpose: str | None
    sources: tuple[ContentSelection, ...]
    needs_review: bool
    attributes: MaterialAttributes


class RefreshRetrieval(CognitiveValue):
    namespace: NamespaceSelector = None
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


class Selector(CognitiveValue):
    kind: Literal["ref", "resource", "tag", "entity", "schema", "object"]
    value: str
    start: int | None = None
    end: int | None = None


class QueryTimeClause(CognitiveValue):
    axis: Literal["occurred", "observed", "formed", "valid", "recorded"]
    domain: str = "utc"
    from_value: str | None = None
    to_value: str | None = None
    at: str | None = None
    within: str | None = None


class QueryControls(CognitiveValue):
    selectors: tuple[Selector, ...] = ()
    limit: int | None = Field(default=None, ge=1)
    return_types: tuple[str, ...] = ()
    times: tuple[TimeCondition, ...] = ()
    time_clauses: tuple[QueryTimeClause, ...] = ()
    filters: CandidateFilter | None = None
    prefer: tuple[str, ...] = ()
    avoid: tuple[str, ...] = ()
    sort: str | None = None
    explore: bool = False
    diagnostics: bool = False
    include_review: bool | None = None
    include_exploration: bool | None = None


class QueryAtom(CognitiveValue):
    kind: Literal["atom"] = "atom"
    text: str
    selectors: tuple[Selector, ...] = ()
    controls: QueryControls = Field(default_factory=QueryControls)
    start: int | None = None
    end: int | None = None


class QueryAny(CognitiveValue):
    kind: Literal["any"] = "any"
    children: tuple[QueryExpression, ...] = Field(min_length=1)
    text: str = ""
    controls: QueryControls = Field(default_factory=QueryControls)


class QueryAll(CognitiveValue):
    kind: Literal["all"] = "all"
    children: tuple[QueryExpression, ...] = Field(min_length=1)
    text: str = ""
    controls: QueryControls = Field(default_factory=QueryControls)


type QueryExpression = Annotated[QueryAtom | QueryAny | QueryAll, Field(discriminator="kind")]


class DeliveryRequest(CognitiveValue):
    purpose: str | None = None
    detail: Literal["concise", "normal", "expanded"] = "normal"
    steward: Literal["off", "optional", "required"] | None = None
    budget: DeliveryBudget | None = None
    output_language: str | None = None


class ContextSelection(CognitiveValue):
    work: WorkRevisionRef | None = None
    fragments: tuple[ContextFragment, ...] = ()
    prior_deliveries: tuple[ResultRef, ...] = ()
    stages: tuple[Literal["retrieval", "localization", "evaluation"], ...] | None = Field(
        default=None, min_length=1
    )


class QueryAnchor(CognitiveValue):
    selections: tuple[MaterialSelection, ...] = ()
    prior_delivery: ResultRef | None = None
    snapshot: str | SnapshotRef | None = None
    view: Literal["current", "anchored"] = "current"


class QueryRequest(CognitiveValue):
    text: str
    namespace: NamespaceSelector = None
    controls: QueryControls = Field(default_factory=QueryControls)
    context: ContextSelection = Field(default_factory=ContextSelection)
    delivery: DeliveryRequest = Field(default_factory=DeliveryRequest)
    anchor: QueryAnchor | None = None
    expression: QueryExpression | None = None
    aspects: tuple[str, ...] = ()
    save: bool = False


class UsedContext(CognitiveValue):
    key: str
    source: MaterialSelection | None = None
    text: str | None = None
    role: str
    stage: str
    parent_key: str
    contribution: float | None = None


class BoundQueryBranch(CognitiveValue):
    path: tuple[int, ...]
    controls: QueryControls
    materials: tuple[MaterialRef, ...] = ()
    concepts: tuple[ConceptRef, ...] = ()


class BoundQueryControls(CognitiveValue):
    controls: QueryControls
    namespace: NamespaceRef
    branches: tuple[BoundQueryBranch, ...] = ()


class QueryIntent(CognitiveValue):
    original_text: str
    resolved_text: str | None = None
    aspects: tuple[str, ...] = ()
    constraints: BoundQueryControls
    context: tuple[UsedContext, ...] = ()
    bound_materials: tuple[MaterialRef, ...] = ()
    bound_concepts: tuple[ConceptRef, ...] = ()
    expression: QueryExpression | None = None


class PassageEvidence(CognitiveValue):
    key: str
    material: MaterialRef
    candidate: CandidateRef | None = None
    fragments: tuple[DeliveredText, ...]
    support_context: tuple[DeliveredText, ...] = ()
    scored_ranges: tuple[ContentSelection, ...] = ()
    shared_context: tuple[ContentSelection, ...] = ()
    scores: tuple[ScoreValue, ...] = ()
    assessment: dict[str, JsonValue] | None = None
    selection_reason: str

    @property
    def selections(self) -> tuple[ContentSelection, ...]:
        """Return every original range needed to read this evidence item independently."""
        return union_selections(
            (
                *self.scored_ranges,
                *(part.content for part in self.fragments),
                *(part.content for part in self.support_context),
                *self.shared_context,
            )
        )


class RetrievedMaterial(CognitiveValue):
    material: CognitiveMaterialView
    rank: int = Field(ge=1)
    basis: RankEvidence | None = None
    passage_keys: tuple[str, ...]
    delivery_status: DeliveryStatus


class ContinuationLink(CognitiveValue):
    target: MaterialRef
    relation: str
    via: tuple[MaterialRef, ...] = ()
    stage: str | None = None
    score: float | None = None
    route: dict[str, str | None] = Field(default_factory=dict)


class DeliveryUsage(CognitiveValue):
    unit: Literal["characters", "bytes"]
    amount: int = Field(ge=0)


class EvidenceDelivery(CognitiveValue):
    intent: QueryIntent
    formed_at: datetime | None = None
    snapshot: SnapshotRef
    captured_generation: int | None
    materials: tuple[RetrievedMaterial, ...]
    passages: tuple[PassageEvidence, ...]
    coverage: SearchCoverage
    branch_coverage: tuple[WaveBranchCoverage, ...] = ()
    continuation: tuple[ContinuationLink, ...] = ()
    program: str
    program_run: ProgramRunFacts | None = None
    budget: DeliveryBudget | None = None
    delivery_usage: DeliveryUsage | None = None
    evaluation: dict[str, JsonValue] | None = None
    issues: tuple[OperationIssue, ...] = ()
    diagnostics: dict[str, JsonValue] | None = None


class SaveEvidence(CognitiveValue):
    """Adopt an already completed delivery, without repeating its query."""

    evidence: EvidenceDelivery
    retain_snapshot: bool = False


class RetrievalOutcome(CognitiveValue):
    evidence: EvidenceDelivery
    evidence_ref: ResultRef | None = None
    reading: ReadingBrief | None = None
    reading_ref: ResultRef | None = None
    issues: tuple[OperationIssue, ...] = ()


QueryAny.model_rebuild()
QueryAll.model_rebuild()
