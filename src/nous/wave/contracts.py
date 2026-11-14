"""User-supplied collection, query and source selection values."""

from __future__ import annotations

from collections.abc import Sequence
from graphlib import CycleError, TopologicalSorter
from heapq import heappop, heappush
from typing import Annotated, Literal

from pydantic import BaseModel, ConfigDict, Field, JsonValue, model_validator

from nous.configuration.metadata import ConfigSequence
from nous.content.contracts import ArtifactRef, ContentSelection, ExternalInput, SourceSelection
from nous.models.vector_contracts import SpaceDefinition, VectorRef
from nous.temporal.values import CognitiveTimes, Interval


class WaveValue(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class PortRef(WaveValue):
    node: str = Field(min_length=1)
    port: str = Field(min_length=1)


class ProgramNode(WaveValue):
    id: str = Field(min_length=1)
    operator: str = Field(min_length=1)
    inputs: dict[str, PortRef | ConfigSequence[PortRef]] = Field(default_factory=dict)
    parameters: dict[str, JsonValue] = Field(default_factory=dict)


class WaveProgram(WaveValue):
    nodes: ConfigSequence[ProgramNode]
    outputs: dict[str, PortRef]


class ManualOwner(WaveValue):
    kind: Literal["manual"] = "manual"


class CognitionOwner(WaveValue):
    kind: Literal["cognition"] = "cognition"
    namespace_id: str = Field(min_length=1)


type SnapshotOwner = Annotated[ManualOwner | CognitionOwner, Field(discriminator="kind")]


class SnapshotRef(WaveValue):
    kind: Literal["snapshot"] = "snapshot"
    instance_id: str
    id: str
    identifier: str


class CandidateRef(WaveValue):
    snapshot: str
    key: str


class SnapshotDescriptor(WaveValue):
    ref: SnapshotRef
    owner: SnapshotOwner
    collection: str
    captured_generation: int | None
    profile_signature: str | None
    space_id: str | None
    space_signature: str
    identity: dict[str, JsonValue]
    metadata: dict[str, JsonValue]


class CandidateDescriptor(WaveValue):
    ref: CandidateRef
    group_key: str
    content: ContentSelection
    retrieval_content: ContentSelection
    origin_contents: tuple[ContentSelection, ...] = Field(min_length=1)


class GroupDescriptor(WaveValue):
    key: str = Field(min_length=1)
    document_key: str = Field(min_length=1)
    title: str | None = None
    context: dict[str, JsonValue] = Field(default_factory=dict)


class ChannelScore(WaveValue):
    channel: str
    rank: int | None = None
    score: float | None = None


class RankEvidence(WaveValue):
    candidate: CandidateRef
    candidate_rank: int = Field(ge=1)
    ranking_score: float
    quality_score: float | None
    score_kind: str
    channels: tuple[ChannelScore, ...] = ()
    details: dict[str, JsonValue] = Field(default_factory=dict)


class DeliveredText(WaveValue):
    content: ContentSelection
    text: str


class ScoreValue(WaveValue):
    name: str
    value: float = Field(allow_inf_nan=False)
    scale: str
    producer: str
    based_on: tuple[str, ...] = ()


class PassageDelivery(WaveValue):
    candidate: CandidateRef
    scored_ranges: tuple[ContentSelection, ...] = Field(min_length=1)
    fragments: tuple[DeliveredText, ...] = Field(min_length=1)
    support_context: tuple[DeliveredText, ...] = ()
    shared_context: tuple[ContentSelection, ...] = ()
    scores: tuple[ScoreValue, ...] = ()
    assessment: dict[str, JsonValue] | None = None
    reason: Literal["body_match", "local_score", "title_preview", "preview"]
    matched_fields: tuple[Literal["body", "title"], ...] = ()
    evidence: RankEvidence | None = None

    @property
    def text(self) -> str:
        return "…".join(fragment.text for fragment in self.fragments)

    @property
    def selections(self) -> tuple[ContentSelection, ...]:
        return tuple(fragment.content for fragment in self.fragments)


type DeliveryStatus = Literal[
    "selected", "budget_exhausted", "overlap", "no_candidates", "not_selected"
]


class DeliveryBudget(WaveValue):
    amount: int = Field(ge=1)
    unit: Literal["characters", "bytes", "tokens"] = "characters"


class GroupHit(WaveValue):
    group: GroupDescriptor
    rank: int = Field(ge=1)
    basis: RankEvidence
    passages: tuple[PassageDelivery, ...]
    delivery_status: DeliveryStatus = "not_selected"


class LaneCoverage(WaveValue):
    examined_rows: int
    eligible_groups_known: int | None
    recall_exhausted: bool
    truncated: bool
    reason: str | None


class SearchCoverage(WaveValue):
    requested_groups: int | None
    returned_groups: int
    eligible_groups_known: int | None
    allowed_groups: int
    recall_exhausted: bool
    truncated: bool
    reason: str | None
    lanes: dict[str, LaneCoverage]
    lane_limit_unit: Literal["groups", "candidates"]
    group_scan_limit: int
    passages_per_group: int
    candidates: int | None
    total_candidates: int
    max_union_candidates: int
    top_k: int
    offered_candidates: int | None
    ranked_candidates: int | None
    unique_candidates: int | None
    candidate_limit_reached: bool
    top_k_truncated: bool


class WaveBranchCoverage(WaveValue):
    name: str
    coverage: SearchCoverage


class CandidateHit(WaveValue):
    basis: RankEvidence
    passage: PassageDelivery


class ProgramRunFacts(WaveValue):
    """The effective program and bounded settings for this particular query."""

    name: str
    signature: str
    definition_signature: str
    requested_program: str | None
    selection: Literal["explicit", "default", "fallback", "exploration"]
    requirements: tuple[str, ...]
    nodes: tuple[ProgramNode, ...]
    outputs: dict[str, JsonValue]
    options: dict[str, JsonValue]
    budget: DeliveryBudget | None
    space_id: str | None
    space_signature: str | None
    space_definition: SpaceDefinition | None


class WaveSearchResult(WaveValue):
    """A complete Wave readout with ranking and delivered content kept distinct."""

    snapshot: SnapshotRef
    program: str
    program_run: ProgramRunFacts | None = None
    score_kind: str
    used_lanes: tuple[str, ...]
    skipped_lanes: tuple[dict[str, JsonValue], ...]
    hits: tuple[CandidateHit, ...]
    groups: tuple[GroupHit, ...]
    grouping: Literal["candidate", "document"]
    rank_scope: str
    context_summary: dict[str, JsonValue]
    cue_summary: tuple[dict[str, JsonValue], ...]
    algorithm_versions: dict[str, str]
    scope: dict[str, JsonValue]
    coverage: SearchCoverage
    branch_coverage: tuple[WaveBranchCoverage, ...] = ()
    thinking: dict[str, JsonValue] | None = None
    diagnostics: dict[str, JsonValue] | None = None
    issues: tuple[dict[str, JsonValue], ...] = ()


class DocumentInput(WaveValue):
    file: str | None = None
    text: str | None = None
    artifact: str | None = None
    selector: SourceSelection = Field(default_factory=SourceSelection)

    @model_validator(mode="after")
    def one_source(self) -> DocumentInput:
        if sum(value is not None for value in (self.file, self.text, self.artifact)) != 1:
            raise ValueError("document input requires exactly one of file, text, artifact")
        return self


class Document(WaveValue):
    key: str = Field(min_length=1)
    input: DocumentInput
    title: str | None = None
    derived_from: tuple[str, ...] = ()
    evidence_group: str | None = Field(default=None, min_length=1)
    times: CognitiveTimes | None = None


def document_order(documents: Sequence[Document]) -> tuple[str, ...]:
    """Visit parents first while retaining each document's source order."""
    parents = {item.key: item.derived_from for item in documents}
    sorter = TopologicalSorter(parents)
    try:
        sorter.prepare()
    except CycleError as error:
        raise ValueError("document derivation contains a cycle") from error
    priority: dict[str, int] = {}
    pending = list(reversed(parents))
    while pending:
        key = pending.pop()
        if key in priority:
            continue
        if key not in parents:
            raise ValueError(f"document references an unknown parent: {key}")
        priority[key] = len(priority)
        pending.extend(reversed(parents[key]))
    ready: list[tuple[int, str]] = []
    for key in sorter.get_ready():
        heappush(ready, (priority[key], key))
    result: list[str] = []
    while ready:
        _, key = heappop(ready)
        result.append(key)
        sorter.done(key)
        for successor in sorter.get_ready():
            heappush(ready, (priority[successor], successor))
    return tuple(result)


class Concept(WaveValue):
    key: str = Field(min_length=1)
    label: str = Field(min_length=1)
    description: str = Field(min_length=1)
    aliases: tuple[str, ...] = ()

    def embedding_text(self) -> str:
        lines = [self.label, self.description]
        lines.extend(alias for alias in self.aliases if alias and alias not in lines)
        return "\n".join(lines)


class SourceSequence(WaveValue):
    document: str
    concepts: tuple[str, ...]
    ordered: bool


class CandidateContent(WaveValue):
    document: str
    selector: SourceSelection = Field(default_factory=SourceSelection)


class PathStep(WaveValue):
    concept: str
    selections: tuple[SourceSelection, ...] = Field(default=(SourceSelection(),), min_length=1)


class CandidatePath(WaveValue):
    key: str = Field(min_length=1)
    label: str = ""
    steps: tuple[PathStep, ...] = Field(min_length=1)


class SupportGroup(WaveValue):
    key: str = Field(min_length=1)
    required_sources: tuple[str, ...] = ()
    basis_refs: tuple[CandidateContent, ...] = ()
    kind: Literal["source", "interpretation", "hypothesis", "imagination"]
    origin_ref: CandidateContent | None = None

    @model_validator(mode="after")
    def evidence_required(self) -> SupportGroup:
        if self.kind in {"source", "interpretation"} and not self.required_sources:
            raise ValueError("source and interpretation require source premises")
        if self.kind == "interpretation" and self.origin_ref is None:
            raise ValueError("interpretation requires its first origin position")
        return self


class CandidateOccurrence(WaveValue):
    key: str
    concept: str
    selections: tuple[SourceSelection, ...] = Field(min_length=1)


class CandidateAnnotation(WaveValue):
    occurrences: tuple[CandidateOccurrence, ...] = ()
    concepts: tuple[str, ...] | None = None
    paths: tuple[CandidatePath, ...] | None = Field(default=None, max_length=16)
    annotation_ref: str | None = None
    annotation_concepts: dict[str, str] = Field(default_factory=dict)

    @model_validator(mode="after")
    def one_path_authority(self) -> CandidateAnnotation:
        if self.occurrences and self.annotation_ref is not None:
            raise ValueError("inline occurrences and annotation_ref are mutually exclusive")
        if self.paths is not None and self.annotation_ref is not None:
            raise ValueError("paths and annotation_ref are mutually exclusive")
        if self.paths and len({path.key for path in self.paths}) != len(self.paths):
            raise ValueError("candidate path keys must be unique")
        return self


class Candidate(CandidateAnnotation):
    kind: Literal["exact"] = "exact"
    key: str = Field(min_length=1)
    content: CandidateContent
    document: str
    group_key: str | None = Field(default=None, min_length=1)


class CompleteBlocks(WaveValue):
    kind: Literal["complete_blocks"] = "complete_blocks"
    target_bytes: int = Field(ge=1)


class DocumentCandidates(CandidateAnnotation):
    """Declare one document's layout; Wave expands it over one prepared text view."""

    kind: Literal["document"] = "document"
    document: str
    group_key: str = Field(min_length=1)
    whole: bool = True
    local_layout: CompleteBlocks | None = None

    @property
    def key(self) -> str:
        return self.group_key

    @property
    def content(self) -> CandidateContent:
        return CandidateContent(document=self.document)


class Association(WaveValue):
    key: str = Field(min_length=1)
    source: str
    target: str
    relation: str = Field(min_length=1)
    directed: bool
    strength: float = Field(ge=0, le=1, allow_inf_nan=False)
    confidence: float = Field(ge=0, le=1, allow_inf_nan=False)
    polarity: Literal["positive", "negative"]
    support_group: str


class CollectionManifest(WaveValue):
    name: str = Field(min_length=1)
    space: str | None = None
    documents: tuple[Document, ...] = ()
    groups: tuple[GroupDescriptor, ...] = ()
    concepts: tuple[Concept, ...] = ()
    source_sequences: tuple[SourceSequence, ...] = ()
    candidates: tuple[Candidate | DocumentCandidates, ...] = ()
    support_groups: tuple[SupportGroup, ...] = ()
    associations: tuple[Association, ...] = ()

    @model_validator(mode="after")
    def references_exist(self) -> CollectionManifest:
        collections = (
            ("document", self.documents),
            ("group", self.groups),
            ("concept", self.concepts),
            ("candidate", self.candidates),
            ("association", self.associations),
            ("support_group", self.support_groups),
        )
        for kind, items in collections:
            keys = [item.key for item in items]
            if len(keys) != len(set(keys)):
                raise ValueError(f"duplicate {kind} key")
        documents = {item.key for item in self.documents}
        concepts = {item.key for item in self.concepts}
        for item in self.documents:
            if set(item.derived_from) - documents:
                raise ValueError(f"document {item.key} references an unknown parent")
            if item.evidence_group and item.derived_from:
                raise ValueError("evidence_group belongs to an original source")
        for item in self.source_sequences:
            if item.document not in documents or set(item.concepts) - concepts:
                raise ValueError("source sequence references an unknown document or concept")
        group_documents: dict[str, str] = {}
        for item in self.candidates:
            group_key = item.group_key or item.document
            if group_key in group_documents and group_documents[group_key] != item.document:
                raise ValueError("a candidate group must belong to one exact document")
            group_documents[group_key] = item.document
            if item.document not in documents or (
                isinstance(item, Candidate) and item.content.document not in documents
            ):
                raise ValueError(f"candidate {item.key} references an unknown document")
            if (
                set(item.concepts or ())
                | {step.concept for path in item.paths or () for step in path.steps}
                | {occurrence.concept for occurrence in item.occurrences}
            ) - concepts:
                raise ValueError(f"candidate {item.key} references an unknown concept")
        for group in self.groups:
            if group_documents.get(group.key) != group.document_key:
                raise ValueError(f"group {group.key} does not match its candidates")
        groups = {group.key for group in self.support_groups}
        for group in self.support_groups:
            if (
                set(group.required_sources) - documents
                or any(ref.document not in documents for ref in group.basis_refs)
                or (group.origin_ref is not None and group.origin_ref.document not in documents)
            ):
                raise ValueError(f"support group {group.key} references an unknown document")
        for item in self.associations:
            if item.source not in concepts or item.target not in concepts:
                raise ValueError(f"association {item.key} references an unknown concept")
            if item.support_group not in groups:
                raise ValueError(f"association {item.key} references an unknown support group")
        document_order(self.documents)
        return self


class WaveBuild(WaveValue):
    manifest: ExternalInput
    documents: dict[str, ExternalInput] = Field(default_factory=dict)
    space: str | None = None


class WaveSnapshotRequest(WaveValue):
    snapshot: str
    concepts: tuple[str, ...] = ()


class WaveRead(WaveValue):
    snapshot: str
    candidate: str
    sources: bool = False


class WaveCleanup(WaveValue):
    keep_recent: int = Field(default=0, ge=0)
    retain: tuple[str | SnapshotRef, ...] = ()


class EvidenceProjection(WaveValue):
    """Fixed source identities and each document's independent-origin membership."""

    roots: dict[str, tuple[ArtifactRef, ...]]
    documents: dict[str, tuple[str, ...]]
    source_documents: tuple[str, ...] = ()

    @model_validator(mode="after")
    def defined_roots(self) -> EvidenceProjection:
        if any(not key or not refs for key, refs in self.roots.items()):
            raise ValueError("evidence roots require an identity and exact artifact references")
        for document, roots in self.documents.items():
            if len(roots) != len(set(roots)) or any(root not in self.roots for root in roots):
                raise ValueError(f"document {document} has undefined or repeated evidence roots")
        if set(self.source_documents) - self.documents.keys():
            raise ValueError("source sequences require a defined source document")
        if any(len(self.documents[key]) != 1 for key in self.source_documents):
            raise ValueError("an original sequence must belong to exactly one independent source")
        return self


class CorpusView(CollectionManifest):
    """A frozen collection of already accepted content for snapshot construction."""

    evidence: EvidenceProjection
    owner: SnapshotOwner = Field(default_factory=ManualOwner)
    generation: int | None = Field(default=None, ge=0)
    profile: str | None = None
    manifest_artifact: ArtifactRef | None = None
    metadata: dict[str, JsonValue] = Field(default_factory=dict)
    capture_stamps: dict[str, str] = Field(default_factory=dict)

    @model_validator(mode="after")
    def accepted_documents(self) -> CorpusView:
        if any(document.input.artifact is None for document in self.documents):
            raise ValueError("a corpus view requires accepted artifact references")
        if set(self.evidence.documents) != {document.key for document in self.documents}:
            raise ValueError("evidence projection must cover exactly the accepted documents")
        source_documents = set(self.evidence.source_documents)
        if any(
            document.derived_from and document.key in source_documents
            for document in self.documents
        ):
            raise ValueError("a derived document cannot contribute an original source sequence")
        return self


class ResolvedBuildProfile(WaveValue):
    """The exact vector space and Native build settings selected for one build."""

    space_id: str | None
    space_definition: SpaceDefinition | None
    build_config: dict[str, JsonValue]


class ProjectionFacts(WaveValue):
    """One document's candidate and graph declarations, without source closure."""

    source_document: bool
    concepts: tuple[Concept, ...] = ()
    candidates: tuple[Candidate | DocumentCandidates, ...] = ()
    source_sequences: tuple[SourceSequence, ...] = ()
    support_groups: tuple[SupportGroup, ...] = ()
    associations: tuple[Association, ...] = ()


class ReusableDocument(WaveValue):
    document: Document
    roots: dict[str, tuple[ArtifactRef, ...]]
    memberships: tuple[str, ...]
    projection: ProjectionFacts


class ProjectionUnit(WaveValue):
    """A reusable document projection and its stable candidate data."""

    signature: str
    input_signature: str
    layout_signature: str | None = None
    capture_stamp: str | None = None
    projection: ProjectionFacts | None = None
    roots: dict[str, JsonValue]
    document_key: str
    document: dict[str, JsonValue]
    candidates: tuple[dict[str, JsonValue], ...]
    candidate_data: tuple[dict[str, JsonValue], ...]
    artifact_ids: tuple[str, ...]
    vector_mappings: tuple[dict[str, JsonValue], ...]
    candidate_rows: tuple[int, ...]


class BuiltProduct(WaveValue):
    """Complete build facts held by the current call until publication."""

    view: CorpusView
    profile: ResolvedBuildProfile
    identity: dict[str, JsonValue]
    component_signature: str
    graph_signature: str
    graph_payload: dict[str, JsonValue]
    index_location: str
    layout: dict[str, JsonValue]
    units: tuple[ProjectionUnit, ...]
    summary: dict[str, JsonValue]
    created_at: str


class WaveBuildResult(WaveValue):
    snapshot: SnapshotRef
    collection: str
    space: str | None
    covered_generation: int | None
    profile: str | None
    metadata: dict[str, JsonValue]
    current: bool
    identity: dict[str, JsonValue]
    capabilities: tuple[str, ...]
    statistics: dict[str, JsonValue]


class EvidenceSource(WaveValue):
    evidence_root: str
    artifacts: tuple[ArtifactRef, ...]


class WaveReadResult(WaveValue):
    snapshot: SnapshotRef
    candidate: CandidateRef
    content: ContentSelection
    origin_contents: tuple[ContentSelection, ...]
    text: str
    group: GroupDescriptor
    concepts: tuple[str, ...]
    paths: tuple[CandidatePath, ...]
    sources: tuple[EvidenceSource, ...] | None = None


class WaveCleanupResult(WaveValue):
    snapshots: tuple[str, ...]
    components: tuple[str, ...]


class ContextSegment(WaveValue):
    start: int
    end: int
    fragments: tuple[str, ...]


class WaveSegmentResult(WaveValue):
    snapshot: SnapshotRef
    segments: tuple[ContextSegment, ...]
    dimensions: int | None


class WaveInspection(WaveValue):
    snapshot: SnapshotRef
    collection: str
    space: SpaceDefinition | None
    identity: dict[str, JsonValue]
    covered_generation: int | None
    profile: str | None
    metadata: dict[str, JsonValue]
    owner: SnapshotOwner
    capabilities: tuple[str, ...]
    statistics: dict[str, JsonValue]
    source_roots: int
    created_at: str


class NeighborhoodNode(WaveValue):
    concept: str
    independent_roots: int
    roots: tuple[str, ...]
    inbound_weight: float
    outbound_weight: float
    outdegree: int
    candidate_coverage: int
    candidates: tuple[str, ...]
    source_witness_groups: tuple[tuple[str, ...], ...]


class NeighborhoodEdge(WaveValue):
    source: str
    target: str
    weight: float


class NeighborhoodAssociation(WaveValue):
    key: str
    relation: str
    source: str
    target: str
    directed: bool
    strength: float
    confidence: float
    polarity: int
    support_group: dict[str, JsonValue]
    evidence_roots: tuple[str, ...]


class ConceptNeighborhood(WaveValue):
    nodes: tuple[NeighborhoodNode, ...]
    edges: tuple[NeighborhoodEdge, ...]
    associations: tuple[NeighborhoodAssociation, ...]
    truncated: bool


class WaveNeighborhoodResult(WaveValue):
    snapshot: SnapshotRef
    concepts: tuple[str, ...]
    neighborhood: ConceptNeighborhood


class ContextContent(WaveValue):
    artifact: str
    selector: SourceSelection = Field(default_factory=SourceSelection)


class ContextFragment(WaveValue):
    key: str = Field(min_length=1)
    role: str = Field(min_length=1)
    stage: str = "retrieval"
    parent_key: str | None = None
    sequence: int
    text: str | None = None
    content: ContextContent | None = None
    weight: float = Field(default=1, ge=0, allow_inf_nan=False)
    relevance: float = Field(default=1, ge=0, le=1, allow_inf_nan=False)
    validity: float = Field(default=1, ge=0, le=1, allow_inf_nan=False)
    distance: float = Field(default=0, ge=0, allow_inf_nan=False)
    half_life: float | None = Field(default=None, gt=0, allow_inf_nan=False)
    time: CognitiveTimes | None = None

    @model_validator(mode="after")
    def one_body(self) -> ContextFragment:
        if (self.text is None) == (self.content is None):
            raise ValueError("context requires exactly one of text or content")
        return self


class SelectedContext(WaveValue):
    key: str
    role: str
    stage: str = "retrieval"
    parent_key: str | None = None
    contribution: float = 0.0
    sequence: int
    weight: float
    decayed_weight: float
    role_weight: float
    content: ContentSelection | None = None
    text: str | None = None
    time: CognitiveTimes | None = None


class QueryCue(WaveValue):
    label: str = Field(min_length=1)
    description: str = Field(min_length=1)
    is_core: bool = False

    def embedding_text(self) -> str:
        return f"{self.label}\n{self.description}"


class TimeCondition(WaveValue):
    axis: Literal["occurred", "observed", "formed", "valid", "recorded"]
    range: Interval


class CandidateFilter(WaveValue):
    candidate_keys: tuple[str, ...] | None = None
    times: tuple[TimeCondition, ...] = ()
    include_unknown: bool = False


class WaveExpression(WaveValue):
    """A bound retrieval term or a Boolean combination of terms."""

    kind: Literal["term", "any", "all"]
    text: str = ""
    candidate_filter: CandidateFilter | None = None
    core_concepts: tuple[str, ...] = ()
    prefer: tuple[str, ...] = ()
    avoid: tuple[str, ...] = ()
    limit: int | None = Field(default=None, ge=1)
    sort: str | None = None
    children: tuple[WaveExpression, ...] = ()

    @model_validator(mode="after")
    def valid_tree(self) -> WaveExpression:
        if self.kind == "term" and self.children:
            raise ValueError("a Wave term cannot have children")
        if self.kind != "term" and not self.children:
            raise ValueError("a Wave combination requires children")
        return self


class ThinkingStage(WaveValue):
    name: str = Field(min_length=1)
    candidates: tuple[str, ...]
    top_k: int = Field(default=10, ge=1)


class CandidateLane(WaveValue):
    score: float = Field(allow_inf_nan=False)
    rank: int = Field(ge=1)


class CandidateSetEntry(WaveValue):
    candidate: str
    lanes: dict[str, CandidateLane] = Field(default_factory=dict)


class CandidateSet(WaveValue):
    rank_scope: Literal["candidate_lane", "grouped_lane"] = "candidate_lane"
    snapshot: str
    entries: tuple[CandidateSetEntry, ...]

    @model_validator(mode="after")
    def unique_candidates(self) -> CandidateSet:
        if len({item.candidate for item in self.entries}) != len(self.entries):
            raise ValueError("candidate set contains duplicate keys")
        return self


class WaveQuery(WaveValue):
    grouping: Literal["candidate", "document"] = "candidate"
    passages_per_group: int = Field(default=1, ge=1, le=10000)
    snapshot: str
    text: str = ""
    aspects: tuple[str, ...] = ()
    expression: WaveExpression | None = None
    vector: VectorRef | ExternalInput | None = None
    candidate_set: CandidateSet | ExternalInput | None = None
    context: tuple[ContextFragment, ...] | ExternalInput = ()
    core_concepts: tuple[str, ...] = ()
    cues: tuple[QueryCue, ...] = ()
    candidate_filter: CandidateFilter | ExternalInput | None = None
    prefer: tuple[str, ...] = ()
    avoid: tuple[str, ...] = ()
    sort: str | None = None
    explore: bool = False
    top_k: int | None = Field(default=None, ge=1)
    delivery_budget: DeliveryBudget | None = None
    program: str | None = None
    include_trace: bool = False
    include_counterevidence: bool | None = None
    thinking_stages: tuple[ThinkingStage, ...] | ExternalInput = ()

    @model_validator(mode="after")
    def unique_context_keys(self) -> WaveQuery:
        if any(not value.strip() for value in self.aspects) or len(set(self.aspects)) != len(
            self.aspects
        ):
            raise ValueError("aspects must be nonempty and unique")
        if isinstance(self.context, ExternalInput):
            return self
        keys = [item.key for item in self.context]
        if len(keys) != len(set(keys)):
            raise ValueError("context keys must be unique")
        return self


class WaveSegment(WaveValue):
    snapshot: str
    fragments: tuple[ContextFragment, ...] | ExternalInput
