"""Public job references and durable maintenance outcomes."""

from dataclasses import dataclass
from datetime import datetime

from pydantic import BaseModel, ConfigDict, JsonValue

from nous.common.operations import OperationIssue


@dataclass(frozen=True)
class QueueDefinition:
    """A registered work name and queue, independent of the payload's domain."""

    kind: str
    queue: str
    workflow: str
    class_name: str
    concurrency: int


class JobRef(BaseModel):
    """Identify a persisted job in one instance, independently of its current state."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    instance_id: str
    workflow_id: str


class JobView(BaseModel):
    """Current durable status, result or failure, and the failed step available for retry."""

    model_config = ConfigDict(frozen=True, extra="forbid", arbitrary_types_allowed=True)

    ref: JobRef
    kind: str
    status: str
    queue_name: str | None = None
    input: dict[str, JsonValue]
    created_at: datetime | None = None
    started_at: datetime | None = None
    completed_at: datetime | None = None
    result: dict[str, JsonValue] | None = None
    error: OperationIssue | None = None
    failed_step: int | None = None
    forked_from: str | None = None
