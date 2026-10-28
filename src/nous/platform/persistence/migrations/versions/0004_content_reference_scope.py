"""Scope content ownership identity by namespace.

Revision ID: 0004_content_reference_scope
Revises: 0003_source_regions
"""

from alembic import op

revision = "0004_content_reference_scope"
down_revision = "0003_source_regions"
branch_labels = None
depends_on = None


def upgrade() -> None:
    with op.batch_alter_table(
        "nous_content_references", naming_convention={"pk": "pk_%(table_name)s"}
    ) as batch:
        batch.drop_constraint("pk_nous_content_references", type_="primary")
        batch.create_primary_key(
            "pk_nous_content_references", ["namespace_id", "owner_ref", "content_hash"]
        )


def downgrade() -> None:
    with op.batch_alter_table("nous_content_references") as batch:
        batch.drop_constraint("pk_nous_content_references", type_="primary")
        batch.create_primary_key("pk_nous_content_references", ["owner_ref", "content_hash"])
