"""Content facts and exact subscriptions; execution state belongs to DBOS."""

from sqlalchemy import JSON, Column, Integer, String, Table

from nous.platform.persistence import metadata

requests = Table(
    "vector_projection_requests",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False, index=True),
    Column("record", JSON, nullable=False),
    Column("commit_seq", Integer, nullable=False),
)

contents = Table(
    "vector_projection_contents",
    metadata,
    Column("key", String, primary_key=True),
    Column("namespace_id", String, nullable=False, index=True),
    Column("record", JSON, nullable=False),
    Column("commit_seq", Integer, nullable=False),
)

claims = Table(
    "vector_projection_claims",
    metadata,
    Column("key", String, primary_key=True),
    Column("request_id", String, nullable=False),
)

subscriptions = Table(
    "vector_projection_subscriptions",
    metadata,
    Column("request_id", String, primary_key=True),
    Column("reference", String, primary_key=True),
    Column("content_key", String, nullable=False, index=True),
)

space_retirements = Table(
    "vector_space_retirements",
    metadata,
    Column("namespace_id", String, primary_key=True),
    Column("space_id", String, primary_key=True),
    Column("commit_seq", Integer, nullable=False),
    Column("reason", String, nullable=False),
)

checks = Table(
    "vector_space_checks", metadata,
    Column("namespace_id", String, primary_key=True),
    Column("caller_id", String, primary_key=True),
    Column("record", JSON, nullable=False),
    Column("commit_seq", Integer, nullable=False),
)
