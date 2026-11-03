"""Authority-owned transaction facts, independent of DBOS scheduling state."""

from sqlalchemy import Column, Integer, MetaData, String, Table

metadata = MetaData()

clock = Table(
    "nous_commit_clock",
    metadata,
    Column("singleton", Integer, primary_key=True),
    Column("commit_seq", Integer, nullable=False),
)

job_links = Table(
    "nous_job_links",
    metadata,
    Column("workflow_id", String, primary_key=True),
    Column("command_digest", String, nullable=False),
    Column("commit_seq", Integer, nullable=False, index=True),
    Column("caller_id", String, nullable=False),
    Column("namespace_id", String, nullable=False),
    Column("scope", String, nullable=False),
    Column("owned_parent", String, index=True),
    Column("workflow_name", String, nullable=False),
    Column("queue_name", String, nullable=False),
)
