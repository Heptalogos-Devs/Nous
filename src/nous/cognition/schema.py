"""Formal written-record identities and immutable revisions."""

import sqlalchemy as sa

from nous.content.schema import artifacts, results

metadata = sa.MetaData()
namespaces = sa.Table(
    "cognition_namespaces",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("name", sa.String, unique=True, nullable=False),
    sa.Column("content_generation", sa.Integer, nullable=False),
    sa.CheckConstraint("content_generation >= 0"),
)
records = sa.Table(
    "written_records",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column(
        "namespace_id", sa.String, sa.ForeignKey(namespaces.c.id), nullable=False, index=True
    ),
    sa.Column("kind", sa.String, nullable=False),
    sa.Column("identifier", sa.String, unique=True, nullable=False),
    sa.Column(
        "head_revision_id",
        sa.String,
        sa.ForeignKey("written_revisions.id", deferrable=True, initially="DEFERRED"),
        nullable=False,
    ),
    sa.Column("archived", sa.Boolean, nullable=False),
    sa.Column("recall", sa.Boolean, nullable=False),
    sa.CheckConstraint("kind IN ('note', 'journal')"),
)
revisions = sa.Table(
    "written_revisions",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("record_id", sa.String, sa.ForeignKey(records.c.id), nullable=False, index=True),
    sa.Column("ordinal", sa.Integer, nullable=False),
    sa.Column("committed_generation", sa.Integer, nullable=False),
    sa.Column("parent_id", sa.String, sa.ForeignKey("written_revisions.id")),
    sa.Column("title", sa.String, nullable=True),
    sa.Column("title_source", sa.String, nullable=False),
    sa.Column("body_artifact_id", sa.String, sa.ForeignKey(artifacts.c.id), nullable=False),
    sa.Column("source_signature", sa.String, nullable=False),
    sa.Column("purpose", sa.String),
    sa.Column("author", sa.String),
    sa.Column("viewpoint", sa.String),
    sa.Column("cognitive_times", sa.JSON, nullable=False),
    sa.Column("change_kind", sa.String, nullable=False),
    sa.Column("reason", sa.String),
    sa.Column("write_key", sa.String),
    sa.Column("write_fingerprint", sa.String),
    sa.Column("origin_result_id", sa.String, sa.ForeignKey(results.c.id)),
    sa.Column("recorded_at", sa.String, nullable=False),
    sa.UniqueConstraint("record_id", "ordinal"),
    sa.UniqueConstraint("record_id", "write_key"),
    sa.CheckConstraint("ordinal > 0"),
)
sources = sa.Table(
    "revision_sources",
    metadata,
    sa.Column("revision_id", sa.String, sa.ForeignKey(revisions.c.id), primary_key=True),
    sa.Column("key", sa.String, primary_key=True),
    sa.Column("ordinal", sa.Integer, nullable=False),
    sa.Column("target_revision_id", sa.String, sa.ForeignKey(revisions.c.id), index=True),
    sa.Column("usage", sa.String, nullable=False),
    sa.Column("name", sa.String),
    sa.UniqueConstraint("revision_id", "ordinal"),
)

source_targets = sa.Table(
    "revision_source_targets",
    metadata,
    sa.Column("revision_id", sa.String, primary_key=True),
    sa.Column("key", sa.String, primary_key=True),
    sa.Column("ordinal", sa.Integer, primary_key=True),
    sa.Column("artifact_id", sa.String, sa.ForeignKey(artifacts.c.id), nullable=False),
    sa.Column("selection", sa.JSON, nullable=False),
    sa.ForeignKeyConstraint(
        ["revision_id", "key"], ["revision_sources.revision_id", "revision_sources.key"]
    ),
)

source_materials = sa.Table(
    "revision_source_materials",
    metadata,
    sa.Column("revision_id", sa.String, primary_key=True),
    sa.Column("key", sa.String, primary_key=True),
    sa.Column("material", sa.JSON, nullable=False),
    sa.ForeignKeyConstraint(
        ["revision_id", "key"], ["revision_sources.revision_id", "revision_sources.key"]
    ),
)

journal_drafts = sa.Table(
    "journal_drafts",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("identifier", sa.String, nullable=False, unique=True),
    sa.Column(
        "namespace_id", sa.String, sa.ForeignKey(namespaces.c.id), nullable=False, index=True
    ),
    sa.Column("result_id", sa.String, sa.ForeignKey(results.c.id), nullable=False),
    sa.Column("composition_result_id", sa.String, sa.ForeignKey(results.c.id)),
    sa.Column("body_artifact_id", sa.String, sa.ForeignKey(artifacts.c.id), nullable=False),
    sa.Column("context", sa.JSON, nullable=False),
    sa.Column("base_revision_id", sa.String, sa.ForeignKey(revisions.c.id)),
    sa.Column("parent_draft_id", sa.String, sa.ForeignKey("journal_drafts.id")),
    sa.Column("adopted_revision_id", sa.String, sa.ForeignKey(revisions.c.id)),
    sa.Column("adopted_as", sa.String),
    sa.Column("recorded_at", sa.String, nullable=False),
)
