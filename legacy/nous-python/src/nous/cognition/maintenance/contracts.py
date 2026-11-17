"""Requests and outcomes for optional post-commit cognitive maintenance."""

from typing import Annotated, Literal

from pydantic import Field

from nous.common.operations import OperationIssue
from nous.runtime.execution import RunningOperationRef
from nous.wave.contracts import SnapshotRef

from ..annotation.contracts import AnnotationNotRequested, AnnotationOutcome
from ..contracts import CognitiveValue, NamespaceRef, RecordRef, RecordRevisionRef
from ..retrieval.contracts import Mode


class MaintainRecord(CognitiveValue):
    target: str | RecordRef | RecordRevisionRef
    namespace: str | NamespaceRef | None = None
    annotation: bool = True
    refresh: bool = False
    mode: Mode | None = None
    task: str | None = None
    require_recall: bool = False
    execution: Literal["wait", "background"] = "wait"


class RefreshNotRequested(CognitiveValue):
    state: Literal["not_requested"] = "not_requested"


class RefreshPublished(CognitiveValue):
    state: Literal["published"] = "published"
    snapshot_ref: SnapshotRef
    captured_generation: int
    current_generation: int
    current: bool


class RefreshFailed(CognitiveValue):
    state: Literal["failed"] = "failed"
    issue: OperationIssue


type RefreshOutcome = Annotated[
    RefreshNotRequested | RefreshPublished | RefreshFailed, Field(discriminator="state")
]


class MaintenanceCompleted(CognitiveValue):
    execution: Literal["completed"] = "completed"
    annotation: AnnotationOutcome = Field(default_factory=AnnotationNotRequested)
    refresh: RefreshOutcome = Field(default_factory=RefreshNotRequested)


class MaintenanceStarted(CognitiveValue):
    execution: Literal["started"] = "started"
    operation_ref: RunningOperationRef


class MaintenanceNotStarted(CognitiveValue):
    execution: Literal["not_started"] = "not_started"
    issue: OperationIssue


type MaintenanceOutcome = Annotated[
    MaintenanceCompleted | MaintenanceStarted | MaintenanceNotStarted,
    Field(discriminator="execution"),
]
