"""Persist reviewable growth runs and authorized repeat policies.

Revision ID: 0010_maintenance
Revises: 0009_credential_events
"""

import sqlalchemy as sa
from alembic import op

revision = "0010_maintenance"
down_revision = "0009_credential_events"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "maintenance_runs",
        sa.Column("id", sa.String, primary_key=True),
        sa.Column("namespace_id", sa.String, nullable=False),
        sa.Column("subject_id", sa.String, nullable=False),
        sa.Column("commit_seq", sa.Integer, nullable=False),
        sa.Column("context", sa.JSON, nullable=False),
        sa.Column("record", sa.JSON, nullable=False),
    )
    op.create_table(
        "maintenance_schedules",
        sa.Column("id", sa.String, primary_key=True),
        sa.Column("namespace_id", sa.String, nullable=False),
        sa.Column("subject_id", sa.String, nullable=False),
        sa.Column("context", sa.JSON, nullable=False),
        sa.Column("record", sa.JSON, nullable=False),
    )
    for table in ("maintenance_runs", "maintenance_schedules"):
        for column in ("namespace_id", "subject_id"):
            op.create_index(f"ix_{table}_{column}", table, [column])


def downgrade() -> None:
    op.drop_table("maintenance_schedules")
    op.drop_table("maintenance_runs")
