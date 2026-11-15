"""Immutable cognitive queries, actual preparation context and delivered selections."""

from datetime import datetime
from typing import Annotated, Literal, NotRequired

from pydantic import Field, JsonValue
from typing_extensions import TypedDict

from nous.cognition.material_contracts import MaterialSelection
from nous.cognition.steward.contracts import ReadingBrief
from nous.common.material_refs import WorkRevisionRef
from nous.common.operations import OperationIssue
from nous.content.contracts import (
    ContentReadView,
    ContentSelection,
    DeliveryBudget,
    MaterialRef,
    MaterialSource,
    ResultRef,
)
from nous.content.selections import union_selections
from nous.wave.contracts import (
    CandidateFilter,
    ContextFragment,
    DeliveryStatus,
    SnapshotRef,
    ThinkingRoute,
    TimeCondition,
)

from ..contracts import (
    CognitiveValue,
    Namespace,
    NamespaceRef,
    NamespaceSelector,
)


class CognitiveGroupContext(CognitiveValue):
    material: MaterialRef
    sources: tuple[ContentSelection, ...]
    needs_review: bool


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
    work: str | WorkRevisionRef | None = None
    fragments: tuple[ContextFragment, ...] = ()
    prior_deliveries: tuple[ResultRef, ...] = ()
    stages: tuple[Literal["retrieval", "localization", "evaluation"], ...] | None = Field(
        default=None, min_length=1
    )


class QueryAnchor(CognitiveValue):
    selections: tuple[MaterialSelection, ...] = ()
    prior_delivery: str | ResultRef | None = None
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


class QueryIntent(CognitiveValue):
    original_text: str
    aspects: tuple[str, ...] = ()
    namespace: NamespaceRef
    controls: QueryControls | None = None
    context: tuple[UsedContext, ...] = ()
    expression: QueryExpression | None = None


class EvidencePassage(CognitiveValue):
    key: str = Field(min_length=1)
    views: tuple[ContentReadView, ...]
    shared: tuple[ContentSelection, ...] = Field(default=(), exclude_if=lambda value: not value)
    assessment: dict[str, JsonValue] | None = Field(
        default=None, exclude_if=lambda value: value is None
    )


class EvidenceMaterial(CognitiveValue):
    material: MaterialRef
    title: str | None = None
    passages: tuple[EvidencePassage, ...] = ()
    rank: int | None = Field(default=None, ge=1)
    score: float | None = None
    score_kind: str | None = None
    delivery_status: DeliveryStatus | None = None
    needs_review: bool | None = None

    def sources(self) -> tuple[MaterialSource, ...]:
        return tuple(
            MaterialSource(
                key=part.key,
                material=self.material,
                selections=union_selections((*(view.source for view in part.views), *part.shared)),
                name=self.title,
            )
            for part in self.passages
        )


class ContinuationLink(CognitiveValue):
    selection: MaterialSelection
    relation: str
    via: tuple[MaterialRef, ...] = ()
    stage: str | None = None
    branch: str | None = None
    score: float | None = None
    route: ThinkingRoute | None = None


class DeliveryUsage(CognitiveValue):
    unit: Literal["characters", "bytes", "tokens"]
    amount: int = Field(ge=0)


class QueryCoverage(TypedDict):
    truncated: bool
    recall_exhausted: bool
    matched: NotRequired[bool]
    reason: NotRequired[str]


class EvidenceDelivery(CognitiveValue):
    intent: QueryIntent
    formed_at: datetime | None = None
    snapshot: SnapshotRef | None = None
    captured_generation: int | None = None
    materials: tuple[EvidenceMaterial, ...] = ()
    coverage: QueryCoverage | None = None
    branch_coverage: dict[str, QueryCoverage] = Field(
        default_factory=dict, exclude_if=lambda value: not value
    )
    continuation: tuple[ContinuationLink, ...] = Field(
        default=(), exclude_if=lambda value: not value
    )
    program: str | None = None
    unavailable_capabilities: tuple[str, ...] = Field(
        default=(), exclude_if=lambda value: not value
    )
    fallback_from: str | None = None
    budget: DeliveryBudget | None = None
    delivery_usage: DeliveryUsage | None = None
    evaluation: dict[str, JsonValue] | None = None
    issues: tuple[OperationIssue, ...] = Field(default=(), exclude_if=lambda value: not value)
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
