"""Explicit feedback and corrections of exact cognitive bases."""

import sqlalchemy as sa

from nous.content.schema import results

from .schema import namespaces, revisions

metadata = sa.MetaData()

invalidations = sa.Table(
    "cognition_basis_invalidations",
    metadata,
    sa.Column("revision_id", sa.String, sa.ForeignKey(revisions.c.id), primary_key=True),
    sa.Column("corrected_revision_id", sa.String, sa.ForeignKey(revisions.c.id), nullable=False),
    sa.Column("reason", sa.String),
    sa.Column("recorded_at", sa.String, nullable=False),
)

feedback = sa.Table(
    "cognition_feedback",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("identifier", sa.String, nullable=False, unique=True),
    sa.Column(
        "namespace_id", sa.String, sa.ForeignKey(namespaces.c.id), nullable=False, index=True
    ),
    sa.Column("target", sa.JSON, nullable=False),
    sa.Column("evidence_result_id", sa.String, sa.ForeignKey(results.c.id)),
    sa.Column("category", sa.String, nullable=False),
    sa.Column("value", sa.String),
    sa.Column("comment", sa.String),
    sa.Column("author", sa.String),
    sa.Column("recorded_at", sa.String, nullable=False),
)

resolutions = sa.Table(
    "cognition_feedback_resolutions",
    metadata,
    sa.Column("feedback_id", sa.String, sa.ForeignKey(feedback.c.id), primary_key=True),
    sa.Column("action", sa.String, nullable=False),
    sa.Column("note", sa.String),
    sa.Column("mapping_note", sa.String),
    sa.Column("applied_revision_id", sa.String, sa.ForeignKey(revisions.c.id)),
    sa.Column("actor", sa.String),
    sa.Column("resolved_at", sa.String, nullable=False),
)
