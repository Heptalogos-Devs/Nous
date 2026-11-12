"""Requests and outcomes for complete written-record and language operations."""

from typing import Annotated, Literal

from pydantic import Field, model_validator

from nous.common.operations import OperationIssue
from nous.content.contracts import (
    ArtifactRef,
    BodySpec,
    ContentReadView,
    ContentSelection,
    ExternalInput,
    ResultOutputRef,
    ResultRef,
    SourceTarget,
)
from nous.content.edits import TextChange
from nous.temporal.values import CognitiveTimes

from ..contracts import (
    CognitiveValue,
    Namespace,
    NamespaceRef,
    NamespaceSelector,
    RecordRef,
    RecordRevisionRef,
    RevisionSelection,
    SourceBinding,
    TitleSource,
)
from ..maintenance.contracts import MaintenanceOutcome

WritingIntent = Literal["new", "edit", "rewrite", "reflection"]


class SourceChanges(CognitiveValue):
    """Describe how to retain, remove and add bindings when creating a revision."""

    preserve: bool = True
    add: tuple[SourceBinding, ...] = ()
    remove: tuple[str, ...] = ()

    def apply_to(self, inherited: tuple[SourceBinding, ...]) -> tuple[SourceBinding, ...]:
        """Apply source changes while preserving retained and added input order.

        Raises:
            NousError: A key is duplicated or a removed key is absent from inherited
                bindings.
        """
        from nous.common.errors import NousError

        original = {source.key for source in inherited}
        additions = [source.key for source in self.add]
        if (
            len(original) != len(inherited)
            or len(additions) != len(set(additions))
            or len(self.remove) != len(set(self.remove))
        ):
            raise NousError("input.duplicate_source", "Source keys must be unique")
        unknown = set(self.remove) - original
        if unknown:
            raise NousError(
                "cognition.source_unknown",
                "Removed source key has no binding",
                details={"keys": sorted(unknown)},
            )
        kept = tuple(
            source for source in inherited if self.preserve and source.key not in self.remove
        )
        duplicate = {source.key for source in kept} & set(additions)
        if duplicate:
            raise NousError(
                "input.duplicate_source",
                "Added source key already has a binding",
                details={"keys": sorted(duplicate)},
            )
        return (*kept, *self.add)


class NormalizedEdit(CognitiveValue):
    start_byte: int = Field(ge=0)
    end_byte: int = Field(ge=0)
    before: str
    after: str


class CitationContext(CognitiveValue):
    inherited_sources: tuple[SourceBinding, ...] = ()
    new_sources: tuple[SourceBinding, ...] = ()
    targets: dict[str, ContentSelection] = Field(default_factory=dict)


class AfterWrite(CognitiveValue):
    """Choose optional work to run after a formal revision commits."""

    annotation: Literal["default", "skip", "run"] = "default"
    refresh: Literal["defer", "run"] = "defer"
    mode: Literal["auto", "lexical", "dense", "tagmemo", "rivermemo", "hybrid", "dtsc"] | None = (
        None
    )
    execution: Literal["wait", "background"] = "wait"

    @model_validator(mode="after")
    def mode_requires_refresh(self) -> AfterWrite:
        if self.mode is not None and self.refresh != "run":
            raise ValueError("A retrieval mode requires refresh=run")
        return self


DEFAULT_AFTER_WRITE = AfterWrite()


class SourceChoice(CognitiveValue):
    target: (
        str
        | RecordRef
        | RecordRevisionRef
        | RevisionSelection
        | ContentSelection
        | ResultOutputRef
        | SourceTarget
    )
    usage: Literal["reference", "current_basis", "inspiration"] = "reference"


type RecordTarget = str | RecordRef | RecordRevisionRef


class CreateJournal(CognitiveValue):
    body: BodySpec
    namespace: NamespaceSelector = None
    title: str | None = None
    purpose: str | None = "record"
    author: str | None = None
    viewpoint: str | None = None
    times: CognitiveTimes | None = None
    sources: tuple[SourceChoice, ...] | ExternalInput = ()
    after: AfterWrite = AfterWrite()


class AddNote(CognitiveValue):
    body: BodySpec
    namespace: NamespaceSelector = None
    title: str | None = None
    purpose: str | None = None
    author: str | None = None
    viewpoint: str | None = None
    times: CognitiveTimes | None = None
    sources: tuple[SourceChoice, ...] | ExternalInput = ()
    remember: bool | None = None
    after: AfterWrite = AfterWrite()


class ReadRecord(CognitiveValue):
    target: RecordTarget
    namespace: NamespaceSelector = None
    sources: bool = False
    blocks: tuple[str, ...] = ()


class ReplaceRecord(CognitiveValue):
    target: RecordTarget
    expected: str | int | RecordRevisionRef | None = None
    body: BodySpec
    namespace: NamespaceSelector = None
    title: str | None = None
    purpose: str | None = None
    author: str | None = None
    viewpoint: str | None = None
    times: CognitiveTimes | None = None
    source_changes: SourceChanges | ExternalInput | None = None
    sources: tuple[SourceChoice, ...] | ExternalInput = ()
    reason: str | None = None
    change_kind: Literal["edit", "correction"] = "edit"
    feedback_refs: tuple[str, ...] = ()
    feedback_mappings: dict[str, str] | ExternalInput = Field(default_factory=dict)
    write_key: str | None = None
    after: AfterWrite = AfterWrite()


class ApplyEdits(CognitiveValue):
    target: RecordTarget
    expected: str | int | RecordRevisionRef | None = None
    changes: tuple[TextChange, ...] | ExternalInput
    namespace: NamespaceSelector = None
    source_changes: SourceChanges | ExternalInput | None = None
    sources: tuple[SourceChoice, ...] | ExternalInput = ()
    reason: str | None = None
    change_kind: Literal["edit", "correction"] = "edit"
    feedback_refs: tuple[str, ...] = ()
    feedback_mappings: dict[str, str] | ExternalInput = Field(default_factory=dict)
    write_key: str | None = None
    after: AfterWrite = AfterWrite()


class RecordMembership(CognitiveValue):
    target: RecordTarget
    expected: str | int | RecordRevisionRef | None = None
    namespace: NamespaceSelector = None
    after: AfterWrite = AfterWrite()


class SetRecall(RecordMembership):
    enabled: bool


class ListRecords(CognitiveValue):
    namespace: NamespaceSelector = None
    kind: Literal["note", "journal"] | None = None
    archived: bool = False
    offset: int = 0
    limit: int | None = None


class DiffRecords(CognitiveValue):
    left: RecordTarget
    right: RecordTarget
    namespace: NamespaceSelector = None


class CommittedRecord(CognitiveValue):
    ref: RecordRef
    revision_ref: RecordRevisionRef
    namespace: Namespace
    title: str | None
    title_source: TitleSource
    changed: bool = True
    committed_generation: int
    in_recall: bool


class RecordOutcome(CognitiveValue):
    """A saved revision and the separately observed post-commit work."""

    commit: CommittedRecord
    maintenance: MaintenanceOutcome
    issues: tuple[OperationIssue, ...] = ()


class RevisionDiff(CognitiveValue):
    left: RecordRevisionRef
    right: RecordRevisionRef
    unified: str


class DraftRef(CognitiveValue):
    kind: Literal["draft"] = "draft"
    instance_id: str
    namespace_id: str
    id: str
    identifier: str


class WritingProductContext(CognitiveValue):
    """Facts required to interpret a completed writing product and form a draft."""

    namespace_ref: NamespaceRef
    intent: WritingIntent
    selected_sources: tuple[SourceBinding, ...]
    base_revision: RecordRevisionRef | None = None
    citation_context: CitationContext = Field(default_factory=CitationContext)
    selected_edit_ranges: tuple[ContentSelection, ...] = ()
    feedback_refs: tuple[str, ...] = ()
    feedback_mappings: dict[str, str] = Field(default_factory=dict)
    instruction: str | None = None
    change_kind: Literal["edit", "correction"] = "edit"
    topic: str | None = None
    purpose: str | None = "record"
    author: str | None = None
    viewpoint: str | None = None
    times: CognitiveTimes
    original_title: str | None = None
    title: str | None = None
    output_state: Literal["material", "no_material"]

    @model_validator(mode="after")
    def intent_base(self) -> WritingProductContext:
        if (self.intent in {"edit", "rewrite"}) != (self.base_revision is not None):
            raise ValueError("Edit/rewrite require an exact base; new/reflection create a Journal")
        return self


class DraftContext(WritingProductContext):
    source_bindings: tuple[SourceBinding, ...] = ()
    normalized_edits: tuple[NormalizedEdit, ...] = ()
    issues: tuple[OperationIssue, ...] = ()


class NewDraftRequest(CognitiveValue):
    intent: Literal["new"] = "new"
    namespace: NamespaceSelector = None
    topic: str | None = None
    sources: tuple[SourceChoice, ...] | ExternalInput = ()
    title: str | None = None
    purpose: str | None = "record"
    author: str | None = None
    viewpoint: str | None = None
    times: CognitiveTimes | None = None
    task: str | None = None
    stream: bool = False
    adoption: Literal["draft", "save"] = "draft"
    after: AfterWrite = AfterWrite()


class EditDraftRequest(CognitiveValue):
    intent: Literal["edit"] = "edit"
    namespace: NamespaceSelector = None
    base: RecordRevisionRef
    targets: tuple[ContentSelection, ...] | ExternalInput
    instruction: str
    sources: tuple[SourceChoice, ...] | ExternalInput = ()
    feedback: tuple[str, ...] = ()
    feedback_mappings: dict[str, str] | ExternalInput = Field(default_factory=dict)
    change_kind: Literal["edit", "correction"] = "edit"
    task: str | None = None
    stream: bool = False
    adoption: Literal["draft", "save"] = "draft"
    after: AfterWrite = AfterWrite()


class RewriteDraftRequest(CognitiveValue):
    intent: Literal["rewrite"] = "rewrite"
    namespace: NamespaceSelector = None
    base: RecordRevisionRef
    instruction: str
    change_kind: Literal["edit", "correction"] = "edit"
    sources: tuple[SourceChoice, ...] | ExternalInput = ()
    task: str | None = None
    stream: bool = False
    adoption: Literal["draft", "save"] = "draft"
    after: AfterWrite = AfterWrite()


class ReflectDraftRequest(CognitiveValue):
    intent: Literal["reflection"] = "reflection"
    namespace: NamespaceSelector = None
    journals: tuple[RecordRevisionRef, ...]
    topic: str
    sources: tuple[SourceChoice, ...] | ExternalInput = ()
    title: str | None = None
    purpose: str | None = "reflection"
    author: str | None = None
    viewpoint: str | None = None
    times: CognitiveTimes | None = None
    task: str | None = None
    stream: bool = False
    adoption: Literal["draft", "save"] = "draft"
    after: AfterWrite = AfterWrite()


type WriteDraft = Annotated[
    NewDraftRequest | EditDraftRequest | RewriteDraftRequest | ReflectDraftRequest,
    Field(discriminator="intent"),
]


class DraftFromResult(CognitiveValue):
    result: ResultRef | str


class ReadDraft(CognitiveValue):
    draft: DraftRef | str
    namespace: NamespaceSelector = None
    sources: bool = False


class ReviseDraft(CognitiveValue):
    draft: DraftRef | str
    namespace: NamespaceSelector = None
    markdown: BodySpec
    sources: SourceChanges | ExternalInput | None = None


class AdoptDraft(CognitiveValue):
    draft: DraftRef | str
    namespace: NamespaceSelector = None
    title: str | None = None
    fork: bool = False
    after: AfterWrite = AfterWrite()


class DraftView(CognitiveValue):
    ref: DraftRef
    result_ref: ResultRef
    composition_result_ref: ResultRef | None = None
    parent_draft: DraftRef | None = None
    body_artifact: ArtifactRef
    body: str
    context: DraftContext
    adopted_revision: RecordRevisionRef | None = None
    adopted_as: Literal["default", "fork"] | None = None
    diff: str | None = None
    source_views: dict[str, tuple[ContentReadView, ...]] = Field(default_factory=dict)


class WritingOutcome(CognitiveValue):
    status: Literal["no_material", "draft", "adopted", "unchanged"]
    product: ResultRef
    draft: DraftRef | None = None
    record: RecordOutcome | None = None
