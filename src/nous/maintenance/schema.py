"""Business proposals and repeat policies; execution state remains in DBOS."""

import sqlalchemy as sa

from nous.platform.persistence import metadata

runs = sa.Table(
    "maintenance_runs",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("namespace_id", sa.String, nullable=False, index=True),
    sa.Column("subject_id", sa.String, nullable=False, index=True),
    sa.Column("commit_seq", sa.Integer, nullable=False),
    sa.Column("context", sa.JSON, nullable=False),
    sa.Column("record", sa.JSON, nullable=False),
)

schedules = sa.Table(
    "maintenance_schedules",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("namespace_id", sa.String, nullable=False, index=True),
    sa.Column("subject_id", sa.String, nullable=False, index=True),
    sa.Column("context", sa.JSON, nullable=False),
    sa.Column("record", sa.JSON, nullable=False),
)
