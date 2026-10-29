"""Authority-owned transaction facts, independent of DBOS scheduling state."""

from sqlalchemy import Column, Integer, MetaData, String, Table, Text, UniqueConstraint

metadata = MetaData()

clock = Table(
    "nous_commit_clock",
    metadata,
    Column("singleton", Integer, primary_key=True),
    Column("commit_seq", Integer, nullable=False),
)

operations = Table(
    "nous_operations",
    metadata,
    Column("operation_key", String, primary_key=True),
    Column("caller_id", String, nullable=False),
    Column("namespace_id", String, nullable=False),
    Column("subject_id", String),
    Column("scope", String, nullable=False),
    Column("operation_id", String, nullable=False),
    Column("operation_kind", String, nullable=False),
    Column("payload_hash", String, nullable=False),
    Column("commit_seq", Integer, nullable=False, unique=True),
    Column("recorded_at", String, nullable=False),
    Column("result_json", Text, nullable=False),
    Column("jobs_json", Text, nullable=False),
    UniqueConstraint(
        "caller_id", "operation_kind", "namespace_id", "subject_id", "scope", "operation_id"
    ),
)

job_links = Table(
    "nous_job_links",
    metadata,
    Column("workflow_id", String, primary_key=True),
    Column("operation_key", String, nullable=False, index=True),
    Column("caller_id", String, nullable=False),
    Column("namespace_id", String, nullable=False),
    Column("subject_id", String),
    Column("scope", String, nullable=False),
    Column("owned_parent", String, index=True),
    Column("workflow_name", String, nullable=False),
    Column("queue_name", String, nullable=False),
)
