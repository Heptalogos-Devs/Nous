"""Public job references and durable maintenance outcomes."""

from datetime import datetime
from typing import Literal

from pydantic import BaseModel, ConfigDict

from nous.cognition.concept_contracts import AnnotationRef
from nous.cognition.contracts import NamespaceRef, RecordRevisionRef
from nous.common.operations import OperationIssue
from nous.content.contracts import ResultRef


class JobRef(BaseModel):
    """Identify a persisted job in one instance, independently of its current state."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    instance_id: str
    workflow_id: str


class JobOutcome(BaseModel):
    """Describe a completed job's business result and any independently queued follow-up."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    state: Literal["ready", "skipped", "superseded", "published_not_current"]
    result_refs: tuple[str, ...] = ()
    related_jobs: tuple[JobRef, ...] = ()
    proposal_result: ResultRef | None = None
    annotation_ref: AnnotationRef | None = None
    activation_state: Literal["active", "inactive"] | None = None
    captured_generation: int | None = None
    covered_generation: int | None = None


class JobView(BaseModel):
    """Current durable status, result or failure, and the failed step available for retry."""

    model_config = ConfigDict(frozen=True, extra="forbid", arbitrary_types_allowed=True)

    ref: JobRef
    kind: Literal["annotation", "refresh"]
    status: str
    queue_name: str | None = None
    namespace_ref: NamespaceRef
    target_revision: RecordRevisionRef | None = None
    created_at: datetime | None = None
    started_at: datetime | None = None
    completed_at: datetime | None = None
    result: JobOutcome | None = None
    error: OperationIssue | None = None
    failed_step: int | None = None
    forked_from: str | None = None
