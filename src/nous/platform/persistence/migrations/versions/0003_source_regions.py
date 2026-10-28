"""Persist exact regions of retained original materials.

Revision ID: 0003_source_regions
Revises: 0002_content
"""

import sqlalchemy as sa
from alembic import op

revision = "0003_source_regions"
down_revision = "0002_content"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "source_regions",
        sa.Column("id", sa.String, primary_key=True),
        sa.Column("namespace_id", sa.String, nullable=False),
        sa.Column("material_id", sa.String, nullable=False),
        sa.Column("record", sa.JSON, nullable=False),
    )
    op.create_index("ix_source_regions_material_id", "source_regions", ["material_id"])


def downgrade() -> None:
    op.drop_table("source_regions")
