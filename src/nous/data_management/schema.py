from __future__ import annotations

from sqlalchemy import JSON, Column, String, Table

from nous.platform.persistence import metadata

bundle_installs = Table(
    "bundle_installs",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False),
    Column("subject_id", String, nullable=False),
    Column("record", JSON, nullable=False),
)


source_operations = Table(
    "source_operations",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False),
    Column("subject_id", String, nullable=False),
    Column("record", JSON, nullable=False),
)
