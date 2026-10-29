"""Content facts and exact subscriptions; execution state belongs to DBOS."""

from sqlalchemy import JSON, Column, Integer, String, Table

from nous.platform.persistence import metadata

requests = Table(
    "vector_projection_requests",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False, index=True),
    Column("subject_id", String, nullable=False),
    Column("record", JSON, nullable=False),
    Column("commit_seq", Integer, nullable=False),
)

contents = Table(
    "vector_projection_contents",
    metadata,
    Column("key", String, primary_key=True),
    Column("namespace_id", String, nullable=False, index=True),
    Column("subject_id", String, nullable=False),
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
