"""Durable import facts; jobs are owned by the execution platform."""

from __future__ import annotations

from sqlalchemy import JSON, Column, String, Table

from nous.platform.persistence import metadata

catalogs = Table(
    "import_catalogs",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False),
    Column("subject_id", String, nullable=False),
    Column("record", JSON, nullable=False),
)

inputs = Table(
    "import_inputs",
    metadata,
    Column("id", String, primary_key=True),
    Column("catalog_id", String, nullable=False, index=True),
    Column("material_id", String, nullable=False),
    Column("record", JSON, nullable=False),
)

units = Table(
    "import_units",
    metadata,
    Column("id", String, primary_key=True),
    Column("catalog_id", String, nullable=False, index=True),
    Column("record", JSON, nullable=False),
)

outputs = Table(
    "import_outputs",
    metadata,
    Column("id", String, primary_key=True),
    Column("unit_id", String, nullable=False, index=True),
    Column("operation_id", String, nullable=False),
    Column("record", JSON, nullable=False),
)

successes = Table(
    "import_file_successes",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False),
    Column("subject_id", String, nullable=False),
    Column("input_source_id", String, nullable=False),
    Column("record", JSON, nullable=False),
)

manifest = Table(
    "import_manifest_heads",
    metadata,
    Column("key", String, primary_key=True),
    Column("success_id", String, nullable=False),
)

requests = Table(
    "import_requests",
    metadata,
    Column("id", String, primary_key=True),
    Column("record", JSON, nullable=False),
)
