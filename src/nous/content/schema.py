"""Tables owned by content records; relationships have one authoritative writer."""

import sqlalchemy as sa

metadata = sa.MetaData()
artifacts = sa.Table(
    "content_artifacts",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("address", sa.String, nullable=False, unique=True),
    sa.Column("media_type", sa.String, nullable=False),
    sa.Column("display_name", sa.String, nullable=False),
    sa.Column("source", sa.String),
    sa.Column("received_at", sa.String),
    sa.Column("formed_at", sa.String),
    sa.Column("recorded_at", sa.String, nullable=False),
)
results = sa.Table(
    "processing_results",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("address", sa.String, nullable=False, unique=True),
    sa.Column("operation", sa.String, nullable=False),
    sa.Column("producer", sa.JSON, nullable=False),
    sa.Column("request", sa.JSON, nullable=False),
    sa.Column("parameters", sa.JSON, nullable=False),
    sa.Column("coverage", sa.JSON, nullable=False),
    sa.Column("formed_at", sa.String, nullable=False),
    sa.Column("recorded_at", sa.String, nullable=False),
)
inputs = sa.Table(
    "processing_inputs",
    metadata,
    sa.Column("result_id", sa.String, sa.ForeignKey("processing_results.id"), primary_key=True),
    sa.Column("ordinal", sa.Integer, primary_key=True),
    sa.Column("artifact_id", sa.String, sa.ForeignKey("content_artifacts.id"), nullable=False),
    sa.Column("key", sa.String, nullable=False),
    sa.Column("selection", sa.JSON, nullable=False),
    sa.Column("representation", sa.String, nullable=False),
)
outputs = sa.Table(
    "processing_outputs",
    metadata,
    sa.Column("result_id", sa.String, sa.ForeignKey("processing_results.id"), primary_key=True),
    sa.Column("ordinal", sa.Integer, primary_key=True),
    sa.Column("name", sa.String, nullable=False),
    sa.Column(
        "artifact_id", sa.String, sa.ForeignKey("content_artifacts.id"), nullable=False, unique=True
    ),
    sa.UniqueConstraint("result_id", "name"),
)
