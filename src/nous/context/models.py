"""Context-owned records; prepared state and admitted state share one lifecycle."""

from sqlalchemy import JSON, Column, String, Table

from nous.platform.persistence import metadata

contexts = Table(
    "cognitive_contexts",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False),
    Column("subject_id", String, nullable=False),
    Column("record", JSON, nullable=False),
)
feedback = Table(
    "context_feedback",
    metadata,
    Column("id", String, primary_key=True),
    Column("context_id", String, nullable=False),
    Column("record", JSON, nullable=False),
)
preparations = Table(
    "context_preparations",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False),
    Column("subject_id", String, nullable=False),
    Column("track_id", String, nullable=False),
    Column("record", JSON, nullable=False),
)
tracks = Table(
    "context_tracks",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False),
    Column("subject_id", String, nullable=False),
    Column("record", JSON, nullable=False),
)
sessions = Table(
    "context_sessions",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False),
    Column("subject_id", String, nullable=False),
    Column("record", JSON, nullable=False),
)
reports = Table(
    "context_reports",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False),
    Column("subject_id", String, nullable=False),
    Column("admission_id", String, nullable=False),
    Column("record", JSON, nullable=False),
)
