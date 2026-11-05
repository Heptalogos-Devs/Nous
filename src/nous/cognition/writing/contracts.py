"""Requests and outcomes for complete written-record and language operations."""

from typing import Literal

from pydantic import Field, model_validator

from nous.common.operations import OperationIssue
from nous.content.contracts import ArtifactRef, ContentReadView, ContentSelection, ResultRef
from nous.temporal.values import CognitiveTimes

from ..contracts import (
    CognitiveValue,
    Namespace,
    RecordRef,
    RecordRevisionRef,
    SourceBinding,
    TitleSource,
)

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


class DraftOverride(CognitiveValue):
    markdown: str
    source_changes: SourceChanges | None = None


class AfterWrite(CognitiveValue):
    """Choose annotation and retrieval work after a formal record commit.

    ``annotation="default"`` follows configuration for changed records;
    ``retrieval="defer"`` leaves retrieval work pending for recalled records.
    A retrieval mode also requests refresh.
    """
    annotation: Literal["default", "skip", "run"] = "default"
    retrieval: Literal["defer", "refresh"] = "defer"
    retrieval_mode: (
        Literal["auto", "lexical", "dense", "tagmemo", "rivermemo", "hybrid", "dtsc"] | None
    ) = None


DEFAULT_AFTER_WRITE = AfterWrite()


class SourceRequest(CognitiveValue):
    target: str | RecordRef | RecordRevisionRef | ArtifactRef | ContentSelection
    usage: Literal["reference", "current_basis", "inspiration"] = "reference"


class CommittedRecord(CognitiveValue):
    ref: RecordRef
    revision_ref: RecordRevisionRef
    namespace: Namespace
    title: str | None
    title_source: TitleSource
    changed: bool = True
    committed_generation: int
    in_recall: bool


class StageOutcome(CognitiveValue):
    state: Literal["not_requested", "pending", "ready", "failed"]
    generation: int | None = None
    ref: str | None = None
    issue: OperationIssue | None = None


class OperationLink(CognitiveValue):
    operation_id: str
    stage: str


class RecordOutcome(CognitiveValue):
    """A committed record and the results of its requested post-commit stages.

    A ``partial`` status identifies a failed follow-up while the record is already
    committed. Stage states and issues identify the work to retry.
    """
    record: CommittedRecord
    status: Literal["success", "partial"]
    annotation: StageOutcome
    retrieval: StageOutcome
    operation: OperationLink


class RevisionDiff(CognitiveValue):
    left: RecordRevisionRef
    right: RecordRevisionRef
    unified: str


class DraftRef(CognitiveValue):
    instance_id: str
    namespace_id: str
    id: str
    identifier: str


class DraftContext(CognitiveValue):
    namespace: Namespace
    intent: WritingIntent
    selected_sources: tuple[SourceBinding, ...]
    source_bindings: tuple[SourceBinding, ...] = ()
    base_revision: RecordRevisionRef | None = None
    citation_context: CitationContext = Field(default_factory=CitationContext)
    normalized_edits: tuple[NormalizedEdit, ...] = ()
    feedback_refs: tuple[str, ...] = ()
    feedback_mappings: dict[str, str] = Field(default_factory=dict)
    instruction: str | None = None
    change_kind: Literal["edit", "correction"] = "edit"
    topic: str | None = None
    purpose: str | None = "record"
    author: str | None = None
    viewpoint: str | None = None
    times: CognitiveTimes
    config_version: int
    task: str

    @model_validator(mode="after")
    def intent_base(self) -> DraftContext:
        if (self.intent in {"edit", "rewrite"}) != (self.base_revision is not None):
            raise ValueError(
                "Edit/rewrite require an exact base; new/reflection create a new Journal"
            )
        if self.normalized_edits and self.intent != "edit":
            raise ValueError("Only edit drafts contain normalized edits")
        return self


class DraftView(CognitiveValue):
    ref: DraftRef
    result_ref: ResultRef
    body_artifact: ArtifactRef
    body: str
    context: DraftContext
    adopted_revision: RecordRevisionRef | None = None
    adopted_as: Literal["generated", "override"] | None = None
    diff: str | None = None
    source_views: dict[str, tuple[ContentReadView, ...]] = Field(default_factory=dict)


class WritingOutcome(CognitiveValue):
    """Result of a writing request, including saved draft and optional adopted record.

    ``no_material`` has no draft; ``draft`` and ``unchanged`` can describe a saved
    draft; ``adopted`` includes a formal record. An unchanged adoption also includes
    its record. Stage results identify completed processing results.
    """
    status: Literal["no_material", "draft", "adopted", "unchanged"]
    draft_ref: DraftRef | None = None
    draft: DraftView | None = None
    record: RecordOutcome | None = None
    stage_results: tuple[str, ...] = ()
