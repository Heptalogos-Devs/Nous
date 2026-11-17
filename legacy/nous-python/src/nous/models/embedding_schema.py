"""Embedding-space and canonical vector-row declarations, without service imports."""

import sqlalchemy as sa

from nous.content.schema import artifacts, results

metadata = sa.MetaData()
spaces = sa.Table(
    "embedding_spaces",
    metadata,
    sa.Column("space_id", sa.String, primary_key=True),
    sa.Column("definition", sa.JSON, nullable=False),
)
vectors = sa.Table(
    "embedding_vectors",
    metadata,
    sa.Column("result_id", sa.String, sa.ForeignKey(results.c.id), primary_key=True),
    sa.Column("space_id", sa.String, sa.ForeignKey(spaces.c.space_id), nullable=False),
    sa.Column("artifact_id", sa.String, sa.ForeignKey(artifacts.c.id), nullable=False),
    sa.Column("purpose", sa.String, nullable=False),
    sa.Column("input_count", sa.Integer, nullable=False),
)
vector_rows = sa.Table(
    "embedding_vector_rows",
    metadata,
    sa.Column("space_id", sa.String, sa.ForeignKey(spaces.c.space_id), primary_key=True),
    sa.Column("purpose", sa.String, primary_key=True),
    sa.Column("input_digest", sa.String(64), primary_key=True),
    sa.Column("result_id", sa.String, sa.ForeignKey(vectors.c.result_id), nullable=False),
    sa.Column("row_index", sa.Integer, nullable=False),
)
