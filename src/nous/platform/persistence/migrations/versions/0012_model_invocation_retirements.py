"""Retain source erasure before a durable invocation begins its paid call.

Revision ID: 0012_model_invocation_retirements
Revises: 0011_model_invocations
"""

import sqlalchemy as sa
from alembic import op

revision = "0012_model_invocation_retirements"
down_revision = "0011_model_invocations"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "nous_model_invocation_retirements",
        sa.Column("key", sa.String, primary_key=True),
        sa.Column("commit_seq", sa.Integer, nullable=False),
        sa.Column("recorded_at", sa.String, nullable=False),
    )


def downgrade() -> None:
    op.drop_table("nous_model_invocation_retirements")
