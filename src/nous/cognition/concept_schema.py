"""Concept identities and immutable meaning versions, plus adopted exact-body annotations."""

import sqlalchemy as sa

from nous.content.schema import artifacts, results

from .schema import namespaces, revisions

metadata = sa.MetaData()
concepts = sa.Table(
    "cognition_concepts",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column(
        "namespace_id", sa.String, sa.ForeignKey(namespaces.c.id), nullable=False, index=True
    ),
    sa.Column("identifier", sa.String, nullable=False, unique=True),
    sa.Column(
        "head_revision_id",
        sa.String,
        sa.ForeignKey("concept_revisions.id", deferrable=True, initially="DEFERRED"),
        nullable=False,
    ),
)
concept_revisions = sa.Table(
    "concept_revisions",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("concept_id", sa.String, sa.ForeignKey(concepts.c.id), nullable=False, index=True),
    sa.Column("ordinal", sa.Integer, nullable=False),
    sa.Column("label", sa.String, nullable=False, index=True),
    sa.Column("description", sa.String, nullable=False),
    sa.Column("aliases", sa.JSON, nullable=False),
    sa.Column("author", sa.String),
    sa.Column("recorded_at", sa.String, nullable=False),
    sa.Column("producer_result_id", sa.String, sa.ForeignKey(results.c.id)),
    sa.Column("body_revision_id", sa.String, sa.ForeignKey(revisions.c.id)),
    sa.UniqueConstraint("concept_id", "ordinal"),
    sa.CheckConstraint("ordinal > 0"),
)
annotations = sa.Table(
    "cognition_annotations",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("identifier", sa.String, nullable=False, unique=True),
    sa.Column(
        "record_revision_id", sa.String, sa.ForeignKey(revisions.c.id), nullable=False, index=True
    ),
    sa.Column("body_artifact_id", sa.String, sa.ForeignKey(artifacts.c.id), nullable=False),
    sa.Column("parser_profile", sa.String, nullable=False),
    sa.Column("proposal_result_id", sa.String, sa.ForeignKey(results.c.id), nullable=False),
    sa.Column("adoption_key", sa.String, nullable=False, unique=True),
    sa.Column("recorded_at", sa.String, nullable=False),
)
annotation_heads = sa.Table(
    "revision_annotation_heads",
    metadata,
    sa.Column("record_revision_id", sa.String, sa.ForeignKey(revisions.c.id), primary_key=True),
    sa.Column("annotation_id", sa.String, sa.ForeignKey(annotations.c.id), nullable=False),
)
bindings = sa.Table(
    "annotation_bindings",
    metadata,
    sa.Column("annotation_id", sa.String, sa.ForeignKey(annotations.c.id), primary_key=True),
    sa.Column("local_key", sa.String, primary_key=True),
    sa.Column(
        "concept_revision_id",
        sa.String,
        sa.ForeignKey(concept_revisions.c.id),
        nullable=False,
        index=True,
    ),
)
occurrences = sa.Table(
    "annotation_occurrences",
    metadata,
    sa.Column("annotation_id", sa.String, primary_key=True),
    sa.Column("key", sa.String, primary_key=True),
    sa.Column("concept_key", sa.String, nullable=False),
    sa.Column("ordinal", sa.Integer, nullable=False),
    sa.ForeignKeyConstraint(
        ["annotation_id", "concept_key"], [bindings.c.annotation_id, bindings.c.local_key]
    ),
    sa.UniqueConstraint("annotation_id", "ordinal"),
)
blocks = sa.Table(
    "annotation_blocks",
    metadata,
    sa.Column("annotation_id", sa.String, primary_key=True),
    sa.Column("occurrence_key", sa.String, primary_key=True),
    sa.Column("ordinal", sa.Integer, primary_key=True),
    sa.Column("block_key", sa.String, nullable=False),
    sa.Column("selection", sa.JSON, nullable=False),
    sa.ForeignKeyConstraint(
        ["annotation_id", "occurrence_key"], [occurrences.c.annotation_id, occurrences.c.key]
    ),
)
paths = sa.Table(
    "annotation_paths",
    metadata,
    sa.Column("annotation_id", sa.String, sa.ForeignKey(annotations.c.id), primary_key=True),
    sa.Column("key", sa.String, primary_key=True),
    sa.Column("label", sa.String, nullable=False),
    sa.Column("ordinal", sa.Integer, nullable=False),
    sa.UniqueConstraint("annotation_id", "ordinal"),
)
path_steps = sa.Table(
    "annotation_path_steps",
    metadata,
    sa.Column("annotation_id", sa.String, primary_key=True),
    sa.Column("path_key", sa.String, primary_key=True),
    sa.Column("ordinal", sa.Integer, primary_key=True),
    sa.Column("occurrence_key", sa.String, nullable=False),
    sa.ForeignKeyConstraint(["annotation_id", "path_key"], [paths.c.annotation_id, paths.c.key]),
    sa.ForeignKeyConstraint(
        ["annotation_id", "occurrence_key"], [occurrences.c.annotation_id, occurrences.c.key]
    ),
)
