"""Nous retains completed business work; DBOS continues to own live execution."""

import sqlalchemy as sa
from dbos import WorkflowStatus
from nous_sdk.contracts import DomainError
from nous_sdk.contracts.execution import ExecutionJob

from nous.platform.persistence import metadata

job_results = sa.Table(
    "nous_job_results",
    metadata,
    sa.Column("workflow_id", sa.String, primary_key=True),
    sa.Column("namespace_id", sa.String, nullable=False, index=True),
    sa.Column("record", sa.JSON, nullable=False),
)

TERMINAL = frozenset({"SUCCESS", "ERROR", "MAX_RECOVERY_ATTEMPTS_EXCEEDED", "CANCELLED"})


def retained_status(job: ExecutionJob) -> WorkflowStatus:
    value = WorkflowStatus()
    value.workflow_id, value.status, value.name = job.workflow_id, job.state, job.workflow_name
    value.class_name = value.config_name = None
    value.authenticated_user = job.caller_id
    value.assumed_role = None
    value.authenticated_roles = None
    value.input = {"args": job.args, "kwargs": job.kwargs}
    value.output = job.result
    value.error = (
        DomainError(job.error.code, job.error.message, details=job.error.data)
        if job.error
        else None
    )
    value.created_at = value.updated_at = job.created_at
    value.queue_name = job.queue_name
    value.executor_id = value.app_version = None
    value.workflow_timeout_ms = value.workflow_deadline_epoch_ms = None
    value.deduplication_id = None
    value.priority = None
    value.queue_partition_key = job.partition_key
    value.forked_from = None
    value.was_forked_from = False
    value.parent_workflow_id = job.owned_parent
    value.dequeued_at = None
    value.delay_until_epoch_ms = job.delay_until_epoch_ms
    value.completed_at = job.completed_at
    value.attributes = {"retained_result": True}
    value.schedule_name = value.application_name = value.app_id = None
    value.recovery_attempts = None
    return value
