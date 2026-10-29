"""Exact derived media and selectors.

Revision ID: 0008_media_derivations
Revises: 0007_reference_directory
"""

import sqlalchemy as sa
from alembic import op

revision = "0008_media_derivations"
down_revision = "0007_reference_directory"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "derived_representations",
        sa.Column("id", sa.String, primary_key=True),
        sa.Column("revision", sa.Integer, primary_key=True),
        sa.Column("namespace_id", sa.String, nullable=False),
        sa.Column("subject_id", sa.String, nullable=False),
        sa.Column("head", sa.Boolean, nullable=False),
        sa.Column("record", sa.JSON, nullable=False),
        sa.Column("commit_seq", sa.Integer, nullable=False),
    )
    op.create_table(
        "derived_regions",
        sa.Column("id", sa.String, primary_key=True),
        sa.Column("namespace_id", sa.String, nullable=False),
        sa.Column("subject_id", sa.String, nullable=False),
        sa.Column("record", sa.JSON, nullable=False),
        sa.Column("commit_seq", sa.Integer, nullable=False),
    )
    for table in ("derived_representations", "derived_regions"):
        for key in ("namespace_id", "subject_id"):
            op.create_index(f"ix_{table}_{key}", table, [key])


def downgrade() -> None:
    op.drop_table("derived_regions")
    op.drop_table("derived_representations")
