"""Persistent work identities, immutable revisions, and actual uses."""

import sqlalchemy as sa

from nous.cognition.schema import namespaces
from nous.content.schema import artifacts, results

metadata = sa.MetaData()
works = sa.Table(
    "cognition_works",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column(
        "namespace_id", sa.String, sa.ForeignKey(namespaces.c.id), nullable=False, index=True
    ),
    sa.Column("identifier", sa.String, nullable=False, unique=True),
    sa.Column(
        "head_revision_id",
        sa.String,
        sa.ForeignKey("work_revisions.id", deferrable=True, initially="DEFERRED"),
        nullable=False,
    ),
)
revisions = sa.Table(
    "work_revisions",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("work_id", sa.String, sa.ForeignKey(works.c.id), nullable=False, index=True),
    sa.Column("ordinal", sa.Integer, nullable=False),
    sa.Column("parent_id", sa.String, sa.ForeignKey("work_revisions.id")),
    sa.Column("state", sa.JSON, nullable=False),
    sa.Column("body_artifact_id", sa.String, sa.ForeignKey(artifacts.c.id), nullable=False),
    sa.Column("recorded_at", sa.String, nullable=False),
    sa.UniqueConstraint("work_id", "ordinal"),
    sa.CheckConstraint("ordinal > 0"),
)
result_dependencies = sa.Table(
    "work_result_dependencies",
    metadata,
    sa.Column("revision_id", sa.String, sa.ForeignKey(revisions.c.id), primary_key=True),
    sa.Column("result_id", sa.String, sa.ForeignKey(results.c.id), primary_key=True),
)
representations = sa.Table(
    "work_representations",
    metadata,
    sa.Column("revision_id", sa.String, sa.ForeignKey(revisions.c.id), primary_key=True),
    sa.Column("artifact_id", sa.String, sa.ForeignKey(artifacts.c.id), nullable=False),
    sa.Column("fields", sa.JSON, nullable=False),
)
uses = sa.Table(
    "work_uses",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("revision_id", sa.String, sa.ForeignKey(revisions.c.id), nullable=False, index=True),
    sa.Column("evidence_id", sa.String, sa.ForeignKey(results.c.id), nullable=False),
    sa.Column("reading_id", sa.String, sa.ForeignKey(results.c.id)),
    sa.Column("selections", sa.JSON, nullable=False),
    sa.Column("purpose", sa.String, nullable=False),
    sa.Column("decision_key", sa.String),
    sa.Column("recorded_at", sa.String, nullable=False),
)
