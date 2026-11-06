"""Wave owns collection and immutable snapshot references; indexes are rebuildable."""

import sqlalchemy as sa

from nous.content.schema import artifacts
from nous.models.embedding_schema import vectors

metadata = sa.MetaData()

collections = sa.Table(
    "wave_collections",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("name", sa.String, nullable=False, unique=True),
    sa.Column("current_snapshot_id", sa.String, nullable=True),
)

snapshots = sa.Table(
    "wave_snapshots",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("identifier", sa.String, nullable=False, unique=True),
    sa.Column("collection_id", sa.String, sa.ForeignKey("wave_collections.id"), nullable=False),
    sa.Column("manifest_artifact_id", sa.String, sa.ForeignKey(artifacts.c.id), nullable=True),
    sa.Column("covered_generation", sa.Integer, nullable=True),
    sa.Column("profile", sa.String, nullable=True),
    sa.Column("space_id", sa.String, nullable=True),
    sa.Column("space_definition", sa.JSON, nullable=True),
    sa.Column("identity", sa.JSON, nullable=False),
    sa.Column("index_location", sa.String, nullable=False),
    sa.Column("mapping", sa.JSON, nullable=False),
    sa.Column("owner", sa.JSON, nullable=False),
    sa.Column("metadata", sa.JSON, nullable=False),
    sa.Column("build_key", sa.String, nullable=True, unique=True),
    sa.Column("build_signature", sa.String, nullable=True),
    sa.Column("build_summary", sa.JSON, nullable=False),
    sa.Column("created_at", sa.String, nullable=False),
)

snapshot_artifacts = sa.Table(
    "wave_snapshot_artifacts",
    metadata,
    sa.Column("snapshot_id", sa.String, sa.ForeignKey("wave_snapshots.id"), primary_key=True),
    sa.Column("artifact_id", sa.String, sa.ForeignKey(artifacts.c.id), primary_key=True),
)

snapshot_vectors = sa.Table(
    "wave_snapshot_vectors",
    metadata,
    sa.Column("snapshot_id", sa.String, sa.ForeignKey("wave_snapshots.id"), primary_key=True),
    sa.Column("kind", sa.String, primary_key=True),
    sa.Column("key", sa.String, primary_key=True),
    sa.Column("input", sa.JSON, nullable=False),
    sa.Column("result_id", sa.String, sa.ForeignKey(vectors.c.result_id), nullable=False),
    sa.Column("row_index", sa.Integer, nullable=False),
)
