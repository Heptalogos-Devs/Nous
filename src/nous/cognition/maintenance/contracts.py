"""Business maintenance plans and durable outcomes."""

from dataclasses import dataclass
from typing import Literal

from pydantic import JsonValue

from nous.common.operations import OperationIssue
from nous.content.contracts import ResultRef
from nous.jobs.contracts import JobRef

from ..annotation.contracts import AnnotationRequest
from ..concept_contracts import AnnotationRef
from ..contracts import CognitiveValue
from ..retrieval.contracts import Mode


class RefreshRequest(CognitiveValue):
    mode: Mode
    space: str | None
    space_definition: dict[str, JsonValue] | None
    include_review: bool
    include_exploration: bool
    candidate_limit: int
    build_config: dict[str, JsonValue]


@dataclass(frozen=True)
class MaintenancePlan:
    """One interpretation of the caller's post-write intent."""

    execution: Literal["inline", "background"]
    annotation: AnnotationRequest | None
    refresh: RefreshRequest | None
    unavailable: tuple[OperationIssue, ...] = ()
    recall_precondition: bool | None = None


class MaintenanceOutcome(CognitiveValue):
    state: Literal["ready", "skipped", "superseded", "published_not_current"]
    result_refs: tuple[str, ...] = ()
    related_jobs: tuple[JobRef, ...] = ()
    proposal_result: ResultRef | None = None
    annotation_ref: AnnotationRef | None = None
    activation_state: Literal["active", "inactive"] | None = None
    captured_generation: int | None = None
    covered_generation: int | None = None
