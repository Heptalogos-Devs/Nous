"""Bind durable model invocations to actual paid call snapshots.

Revision ID: 0011_model_invocations
Revises: 0010_maintenance
"""

import sqlalchemy as sa
from alembic import op

revision = "0011_model_invocations"
down_revision = "0010_maintenance"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "nous_model_invocations",
        sa.Column("key", sa.String, primary_key=True),
        sa.Column("call_id", sa.String, nullable=False),
        sa.Column("commit_seq", sa.Integer, nullable=False),
    )


def downgrade() -> None:
    op.drop_table("nous_model_invocations")
