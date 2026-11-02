"""Session state, independent Work revisions and actual call usage events."""

from sqlalchemy import JSON, Boolean, Column, Integer, String, Table
from nous.platform.persistence import metadata

sessions = Table(
    "context_sessions",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False),
    Column("record", JSON, nullable=False),
)
works = Table(
    "context_work",
    metadata,
    Column("namespace_id", String, primary_key=True),
    Column("caller_id", String, primary_key=True),
    Column("id", String, primary_key=True),
    Column("revision", Integer, primary_key=True),
    Column("head", Boolean, nullable=False),
    Column("record", JSON, nullable=False),
)
feedback = Table(
    "context_feedback",
    metadata,
    Column("id", String, primary_key=True),
    Column("context_id", String, nullable=False),
    Column("namespace_id", String, nullable=False),
    Column("record", JSON, nullable=False),
)
