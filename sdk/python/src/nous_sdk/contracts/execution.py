"""Portable business work and retained results for Namespace archives."""

from typing import Literal

from pydantic import Field, JsonValue

from .common import Contract

JobState = Literal[
    "PENDING",
    "SUCCESS",
    "ERROR",
    "MAX_RECOVERY_ATTEMPTS_EXCEEDED",
    "CANCELLED",
    "ENQUEUED",
    "DELAYED",
]


class ExecutionFailure(Contract):
    name: str
    code: str
    message: str
    data: JsonValue = None


class ExecutionJob(Contract):
    workflow_id: str
    workflow_name: str
    queue_name: str
    caller_id: str
    operation_key: str
    scope: str
    owned_parent: str | None = None
    args: tuple[JsonValue, ...]
    kwargs: dict[str, JsonValue]
    state: JobState
    result: JsonValue = None
    error: ExecutionFailure | None = None
    partition_key: str | None = None
    delay_until_epoch_ms: int | None = None
    created_at: int | None = None
    completed_at: int | None = None


class ExecutionSnapshot(Contract):
    namespace_id: str
    authority_commit_seq: int = Field(ge=0)
    jobs: tuple[ExecutionJob, ...]
