"""Public immutable values for written cognitive records."""

from datetime import datetime
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, computed_field

from nous.content.contracts import ArtifactRef, ContentReadView, ContentSelection
from nous.temporal.values import CognitiveTimes

TitleSource = Literal["user", "heading", "none"]


class CognitiveValue(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")


class NamespaceRef(CognitiveValue):
    """Stable identity of a namespace within an instance."""

    instance_id: str
    id: str


class Namespace(CognitiveValue):
    ref: NamespaceRef
    name: str
    content_generation: int = 0

    @property
    def id(self) -> str:
        return self.ref.id


class RecordRef(CognitiveValue):
    instance_id: str
    namespace_id: str
    kind: Literal["note", "journal"]
    id: str
    identifier: str


class RecordRevisionRef(CognitiveValue):
    record_ref: RecordRef
    revision_id: str
    ordinal: int = Field(ge=1)

    @property
    def identifier(self) -> str:
        return f"{self.record_ref.identifier}@{self.ordinal}"


class RevisionSelection(CognitiveValue):
    """Selections in the body of one exact, formally adopted revision."""

    revision: RecordRevisionRef
    selections: tuple[ContentSelection, ...] = Field(min_length=1)


class SourceBinding(CognitiveValue):
    key: str = Field(min_length=1)
    selections: tuple[ContentSelection, ...] = Field(min_length=1)
    owner_revision: RecordRevisionRef | None = None
    usage: Literal["reference", "current_basis", "inspiration"] = "reference"
    name: str | None = None


class RevisionSummary(CognitiveValue):
    revision_ref: RecordRevisionRef
    committed_generation: int
    title: str | None
    title_source: TitleSource
    recorded_at: datetime
    change_kind: str
    reason: str | None = None


class RecordView(CognitiveValue):
    ref: RecordRef
    revision_ref: RecordRevisionRef
    committed_generation: int
    current_revision: RecordRevisionRef
    namespace: Namespace
    title: str | None
    title_source: TitleSource
    body_artifact: ArtifactRef
    body: str
    selection: RevisionSelection

    @computed_field
    @property
    def body_selection(self) -> ContentSelection:
        return ContentSelection(artifact=self.body_artifact)

    purpose: str | None = None
    author: str | None = None
    viewpoint: str | None = None
    times: CognitiveTimes
    sources: tuple[SourceBinding, ...] = ()
    source_views: dict[str, tuple[ContentReadView, ...]] = Field(default_factory=dict)
    archived: bool
    recall: bool
    change_kind: str
    reason: str | None = None
    origin_result_id: str | None = None
