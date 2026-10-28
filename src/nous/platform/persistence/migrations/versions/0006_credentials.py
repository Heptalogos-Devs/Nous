"""Persist hashed API credentials and their scoped public authority.

Revision ID: 0006_credentials
Revises: 0005_context
"""

import sqlalchemy as sa
from alembic import op

revision = "0006_credentials"
down_revision = "0005_context"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "nous_api_credentials",
        sa.Column("id", sa.String, primary_key=True),
        sa.Column("token_hash", sa.String, unique=True, nullable=False),
        sa.Column("record", sa.JSON, nullable=False),
    )


def downgrade() -> None:
    op.drop_table("nous_api_credentials")
