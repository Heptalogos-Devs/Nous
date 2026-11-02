"""Authoritative cognitive revisions and adopted model-space vectors."""

from sqlalchemy import JSON, Boolean, Column, Index, Integer, String, Table

from nous.platform.persistence import metadata


revisions = Table(
    "cognitive_revisions",
    metadata,
    Column("id", String, primary_key=True),
    Column("revision", Integer, primary_key=True),
    Column("namespace_id", String, nullable=False, index=True),
    Column("kind", String, nullable=False, index=True),
    Column("head", Boolean, nullable=False),
    Column("record", JSON, nullable=False),
    Column("commit_seq", Integer, nullable=False),
)
Index(
    "ix_cognitive_directory_scope_order",
    revisions.c.namespace_id,
    revisions.c.commit_seq,
    revisions.c.id,
    revisions.c.revision,
)
Index(
    "ix_cognitive_directory_scope_kind",
    revisions.c.namespace_id,
    revisions.c.kind,
    revisions.c.id,
    revisions.c.revision,
)
vectors = Table(
    "cognitive_vectors",
    metadata,
    Column("reference", String, primary_key=True),
    Column("space", String, primary_key=True),
    Column("model", String, nullable=False),
    Column("dimensions", Integer, nullable=False),
    Column("vector", JSON, nullable=False),
    Column("call_id", String),
    Column("commit_seq", Integer, nullable=False),
)
