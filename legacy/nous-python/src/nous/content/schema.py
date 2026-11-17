"""Tables owned by content records; relationships have one authoritative writer."""

import sqlalchemy as sa

metadata = sa.MetaData()
artifacts = sa.Table(
    "content_artifacts",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("identifier", sa.String, nullable=False, unique=True),
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
    sa.Column("identifier", sa.String, nullable=False, unique=True),
    sa.Column("operation", sa.String, nullable=False),
    sa.Column("producer", sa.JSON, nullable=False),
    sa.Column("request", sa.JSON, nullable=False),
    sa.Column("parameters", sa.JSON, nullable=False),
    sa.Column("coverage", sa.JSON, nullable=False),
    sa.Column("selected_facts", sa.JSON, nullable=False),
    sa.Column("formed_at", sa.String, nullable=False),
    sa.Column("recorded_at", sa.String, nullable=False),
)
sa.Index(
    "uq_processing_result_cache",
    results.c.operation,
    sa.text("json_extract(parameters, '$.cache_key')"),
    unique=True,
    sqlite_where=sa.text("json_extract(parameters, '$.cache_key') IS NOT NULL"),
)
selected_sources = sa.Table(
    "processing_selected_sources",
    metadata,
    sa.Column("result_id", sa.String, sa.ForeignKey(results.c.id), primary_key=True),
    sa.Column("slot_key", sa.String, primary_key=True),
    sa.Column("ordinal", sa.Integer, nullable=False),
    sa.Column("role", sa.String, nullable=False),
    sa.Column("name", sa.String),
    sa.UniqueConstraint("result_id", "ordinal"),
)
selected_targets = sa.Table(
    "processing_selected_targets",
    metadata,
    sa.Column("result_id", sa.String, primary_key=True),
    sa.Column("slot_key", sa.String, primary_key=True),
    sa.Column("ordinal", sa.Integer, primary_key=True),
    sa.Column("artifact_id", sa.String, sa.ForeignKey(artifacts.c.id), nullable=False),
    sa.Column("selector", sa.JSON, nullable=False),
    sa.ForeignKeyConstraint(
        ["result_id", "slot_key"],
        ["processing_selected_sources.result_id", "processing_selected_sources.slot_key"],
    ),
)
sa.Index("ix_processing_selected_artifact", selected_targets.c.artifact_id)
inputs = sa.Table(
    "processing_inputs",
    metadata,
    sa.Column("result_id", sa.String, sa.ForeignKey("processing_results.id"), primary_key=True),
    sa.Column("group_ordinal", sa.Integer, primary_key=True),
    sa.Column("key", sa.String, nullable=False),
    sa.Column("slot_key", sa.String, nullable=False),
    sa.Column("role", sa.String, nullable=False),
    sa.Column("name", sa.String, nullable=False),
    sa.Column("context", sa.JSON),
    sa.UniqueConstraint("result_id", "key"),
)
parts = sa.Table(
    "processing_input_parts",
    metadata,
    sa.Column("result_id", sa.String, primary_key=True),
    sa.Column("group_ordinal", sa.Integer, primary_key=True),
    sa.Column("part_ordinal", sa.Integer, primary_key=True),
    sa.Column("artifact_id", sa.String, sa.ForeignKey("content_artifacts.id"), nullable=False),
    sa.Column("selector", sa.JSON, nullable=False),
    sa.Column("representation", sa.String, nullable=False),
    sa.Column("media_type", sa.String),
    sa.Column("source_media_type", sa.String),
    sa.ForeignKeyConstraint(
        ["result_id", "group_ordinal"],
        ["processing_inputs.result_id", "processing_inputs.group_ordinal"],
    ),
)
origins = sa.Table(
    "processing_part_origins",
    metadata,
    sa.Column("result_id", sa.String, primary_key=True),
    sa.Column("group_ordinal", sa.Integer, primary_key=True),
    sa.Column("part_ordinal", sa.Integer, primary_key=True),
    sa.Column("origin_ordinal", sa.Integer, primary_key=True),
    sa.Column("artifact_id", sa.String, sa.ForeignKey("content_artifacts.id"), nullable=False),
    sa.Column("selector", sa.JSON, nullable=False),
    sa.Column("precision", sa.String, nullable=False),
    sa.Column("transform_result_id", sa.String, sa.ForeignKey("processing_results.id")),
    sa.ForeignKeyConstraint(
        ["result_id", "group_ordinal", "part_ordinal"],
        [
            "processing_input_parts.result_id",
            "processing_input_parts.group_ordinal",
            "processing_input_parts.part_ordinal",
        ],
    ),
)
sa.Index("ix_processing_parts_artifact", parts.c.artifact_id)
sa.Index("ix_processing_origins_artifact", origins.c.artifact_id)
outputs = sa.Table(
    "processing_outputs",
    metadata,
    sa.Column("result_id", sa.String, sa.ForeignKey("processing_results.id"), primary_key=True),
    sa.Column("ordinal", sa.Integer, primary_key=True),
    sa.Column("name", sa.String, nullable=False),
    sa.Column("role", sa.String, nullable=False),
    sa.Column("representation_kind", sa.String),
    sa.Column(
        "artifact_id", sa.String, sa.ForeignKey("content_artifacts.id"), nullable=False, unique=True
    ),
    sa.UniqueConstraint("result_id", "name"),
)

output_mappings = sa.Table(
    "content_output_mappings",
    metadata,
    sa.Column("result_id", sa.String, primary_key=True),
    sa.Column("output_name", sa.String, primary_key=True),
    sa.Column("ordinal", sa.Integer, primary_key=True),
    sa.Column("selection", sa.JSON, nullable=False),
    sa.Column("relation", sa.String, nullable=False),
    sa.Column("origins", sa.JSON, nullable=False),
    sa.ForeignKeyConstraint(
        ["result_id", "output_name"], ["processing_outputs.result_id", "processing_outputs.name"]
    ),
    sa.CheckConstraint("relation IN ('identity', 'atomic', 'derived')"),
)

dependencies = sa.Table(
    "content_result_dependencies",
    metadata,
    sa.Column("result_id", sa.String, sa.ForeignKey(results.c.id), primary_key=True),
    sa.Column("ordinal", sa.Integer, primary_key=True),
    sa.Column("artifact_id", sa.String, sa.ForeignKey(artifacts.c.id)),
    sa.Column("dependency_result_id", sa.String, sa.ForeignKey(results.c.id)),
    sa.CheckConstraint("(artifact_id IS NULL) != (dependency_result_id IS NULL)"),
)
