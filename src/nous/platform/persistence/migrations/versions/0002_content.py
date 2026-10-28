"""Add authoritative immutable-content ownership references.

Revision ID: 0002_content
Revises: 0001_authority
"""

import sqlalchemy as sa
from alembic import op

revision = "0002_content"
down_revision = "0001_authority"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "nous_content_references",
        sa.Column("owner_ref", sa.String, primary_key=True),
        sa.Column("content_hash", sa.String, primary_key=True),
        sa.Column("namespace_id", sa.String, nullable=False),
        sa.Column("byte_len", sa.Integer, nullable=False),
        sa.Column("media_type", sa.String, nullable=False),
        sa.Column("commit_seq", sa.Integer, nullable=False),
    )
    op.create_index(
        "ix_nous_content_references_content_hash", "nous_content_references", ["content_hash"]
    )
    op.create_index(
        "ix_nous_content_references_namespace_id", "nous_content_references", ["namespace_id"]
    )


def downgrade() -> None:
    op.drop_table("nous_content_references")
