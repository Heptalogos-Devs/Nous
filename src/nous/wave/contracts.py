"""User-supplied collection, query and source selection values."""

from __future__ import annotations

from typing import Annotated, Literal

from pydantic import BaseModel, ConfigDict, Field, JsonValue, model_validator

from nous.application.embedding import VectorRef
from nous.content.contracts import ArtifactRef, ContentSelection, SourceSelection
from nous.temporal.values import CognitiveTimes, Interval


class WaveValue(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class ManualOwner(WaveValue):
    kind: Literal["manual"] = "manual"


class CognitionOwner(WaveValue):
    kind: Literal["cognition"] = "cognition"
    namespace_id: str = Field(min_length=1)


type SnapshotOwner = Annotated[ManualOwner | CognitionOwner, Field(discriminator="kind")]


class SnapshotRef(WaveValue):
    instance_id: str
    id: str
    address: str


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
    owner: SnapshotOwner
    group_key: str
    content: ContentSelection
    origin_contents: tuple[ContentSelection, ...] = Field(min_length=1)
    metadata: dict[str, JsonValue]


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
    candidates: int
    total_candidates: int
    max_union_candidates: int
    top_k: int
    offered_candidates: int
    ranked_candidates: int
    unique_candidates: int
    candidate_limit_reached: bool
    top_k_truncated: bool


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
    derived_from: tuple[str, ...] = ()
    evidence_group: str | None = Field(default=None, min_length=1)
    times: CognitiveTimes | None = None


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
    metadata: dict[str, JsonValue] = Field(default_factory=dict)

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
    concepts: tuple[Concept, ...] = ()
    source_sequences: tuple[SourceSequence, ...] = ()
    candidates: tuple[Candidate | DocumentCandidates, ...] = ()
    support_groups: tuple[SupportGroup, ...] = ()
    associations: tuple[Association, ...] = ()

    @model_validator(mode="after")
    def references_exist(self) -> CollectionManifest:
        collections = (
            ("document", self.documents),
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
            if item.document not in documents or item.content.document not in documents:
                raise ValueError(f"candidate {item.key} references an unknown document")
            if (
                set(item.concepts or ())
                | {step.concept for path in item.paths or () for step in path.steps}
                | {occurrence.concept for occurrence in item.occurrences}
            ) - concepts:
                raise ValueError(f"candidate {item.key} references an unknown concept")
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
        parents = {item.key: item.derived_from for item in self.documents}
        complete: set[str] = set()
        visiting: set[str] = set()

        def visit(key: str) -> None:
            if key in visiting:
                raise ValueError("document derivation contains a cycle")
            if key in complete:
                return
            visiting.add(key)
            for parent in parents[key]:
                visit(parent)
            visiting.remove(key)
            complete.add(key)

        for key in parents:
            visit(key)
        return self


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
            if len(roots) != len(set(roots)) or set(roots) - self.roots.keys():
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

    @model_validator(mode="after")
    def accepted_documents(self) -> CorpusView:
        if any(document.input.artifact is None for document in self.documents):
            raise ValueError("a corpus view requires accepted artifact references")
        if set(self.evidence.documents) != {document.key for document in self.documents}:
            raise ValueError("evidence projection must cover exactly the accepted documents")
        if any(
            document.derived_from and document.key in self.evidence.source_documents
            for document in self.documents
        ):
            raise ValueError("a derived document cannot contribute an original source sequence")
        return self


class ContextContent(WaveValue):
    artifact: str
    selector: SourceSelection = Field(default_factory=SourceSelection)


class ContextFragment(WaveValue):
    key: str = Field(min_length=1)
    role: str = Field(min_length=1)
    sequence: int
    text: str | None = None
    content: ContextContent | None = None
    weight: float = Field(default=1, ge=0, allow_inf_nan=False)
    time: CognitiveTimes | None = None

    @model_validator(mode="after")
    def one_body(self) -> ContextFragment:
        if (self.text is None) == (self.content is None):
            raise ValueError("context requires exactly one of text or content")
        return self


class SelectedContext(WaveValue):
    key: str
    role: str
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


class ThinkingStage(WaveValue):
    name: str = Field(min_length=1)
    candidates: tuple[str, ...]
    top_k: int = Field(default=10, ge=1)


class CandidateLane(WaveValue):
    score: float = Field(allow_inf_nan=False)
    rank: int = Field(ge=1)


class CandidatePoolEntry(WaveValue):
    candidate: str
    lanes: dict[str, CandidateLane] = Field(default_factory=dict)


class CandidatePool(WaveValue):
    rank_scope: Literal["candidate_lane", "grouped_lane"] = "candidate_lane"
    snapshot: str
    entries: tuple[CandidatePoolEntry, ...]

    @model_validator(mode="after")
    def unique_candidates(self) -> CandidatePool:
        if len({item.candidate for item in self.entries}) != len(self.entries):
            raise ValueError("candidate pool contains duplicate keys")
        return self


class WaveQuery(WaveValue):
    grouping: Literal["candidate", "document"] = "candidate"
    passages_per_group: int = Field(default=1, ge=1, le=10000)
    snapshot: str
    text: str = ""
    vector: VectorRef | None = None
    candidate_pool: CandidatePool | None = None
    context: tuple[ContextFragment, ...] = ()
    core_concepts: tuple[str, ...] = ()
    cues: tuple[QueryCue, ...] = ()
    candidate_filter: CandidateFilter | None = None
    top_k: int | None = Field(default=None, ge=1)
    mode: Literal["lexical", "dense", "tagmemo", "rivermemo", "hybrid", "dtsc"] | None = None
    include_trace: bool = False
    include_counterevidence: bool | None = None
    thinking_stages: tuple[ThinkingStage, ...] = ()

    @model_validator(mode="after")
    def unique_context_keys(self) -> WaveQuery:
        keys = [item.key for item in self.context]
        if len(keys) != len(set(keys)):
            raise ValueError("context keys must be unique")
        return self
