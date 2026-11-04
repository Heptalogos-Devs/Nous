"""Public immutable values for written cognitive records."""

from datetime import datetime
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, JsonValue

from nous.content.contracts import ArtifactRef, ContentSelection, ResultRef
from nous.temporal.values import CognitiveTimes


class CognitiveValue(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")


class Namespace(CognitiveValue):
    id: str
    name: str
    content_generation: int = 0


class RecordRef(CognitiveValue):
    instance_id: str
    namespace_id: str
    kind: Literal["note", "journal"]
    id: str
    address: str


class RecordRevisionRef(CognitiveValue):
    record_ref: RecordRef
    revision_id: str
    ordinal: int = Field(ge=1)

    @property
    def address(self) -> str:
        return f"{self.record_ref.address}@{self.ordinal}"


class SourceBinding(CognitiveValue):
    key: str = Field(min_length=1)
    content_selection: ContentSelection
    owner_revision: RecordRevisionRef | None = None
    usage: Literal["reference", "current_basis", "inspiration"] = "reference"
    name: str | None = None


class SavedRecord(CognitiveValue):
    ref: RecordRef
    revision_ref: RecordRevisionRef
    title: str
    namespace: Namespace
    generation: int
    changed: bool = True
    saved: bool = True
    retrieval_state: Literal["pending", "not_in_scope", "ready", "failed"]
    indexed: bool = False
    status: Literal["saved", "partial"] = "saved"
    retrieval_error: dict[str, JsonValue] | None = None
    retrieval_snapshot: str | None = None
    covered_generation: int | None = None
    annotation: Literal["not_requested", "adopted", "failed"] = "not_requested"
    annotation_ref: str | None = None
    annotation_result: str | None = None
    annotation_error: dict[str, JsonValue] | None = None


class RevisionSummary(CognitiveValue):
    revision_ref: RecordRevisionRef
    title: str
    recorded_at: datetime
    change_kind: str
    reason: str | None = None


class RecordView(CognitiveValue):
    ref: RecordRef
    revision_ref: RecordRevisionRef
    current_revision: RecordRevisionRef
    namespace: Namespace
    title: str
    body_artifact: ArtifactRef
    body: str
    purpose: str | None = None
    author: str | None = None
    viewpoint: str | None = None
    times: CognitiveTimes
    sources: tuple[SourceBinding, ...] = ()
    source_texts: dict[str, str] = Field(default_factory=dict)
    archived: bool
    recall: bool
    change_kind: str
    reason: str | None = None
    origin_result_id: str | None = None


class DraftRef(CognitiveValue):
    instance_id: str
    namespace_id: str
    id: str
    address: str


class DraftContext(CognitiveValue):
    namespace: Namespace
    selected_sources: tuple[SourceBinding, ...]
    source_bindings: tuple[SourceBinding, ...] = ()
    target_revision: RecordRevisionRef | None = None
    topic: str | None = None
    purpose: str = "record"
    author: str | None = None
    viewpoint: str | None = None
    times: CognitiveTimes
    config_version: int
    task: str


class DraftView(CognitiveValue):
    ref: DraftRef
    result_ref: ResultRef
    body_artifact: ArtifactRef
    body: str
    context: DraftContext
    adopted_revision: RecordRevisionRef | None = None
    source_texts: dict[str, str] = Field(default_factory=dict)


class JournalizeOutcome(CognitiveValue):
    status: Literal["no_material", "draft", "adopted"]
    draft_ref: DraftRef | None = None
    draft: DraftView | None = None
    record: SavedRecord | None = None
    stage_results: tuple[str, ...] = ()
