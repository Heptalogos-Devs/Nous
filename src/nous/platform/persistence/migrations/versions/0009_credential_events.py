"""Record credential lifecycle facts without storing bearer secrets.

Revision ID: 0009_credential_events
Revises: 0008_media_derivations
"""

import sqlalchemy as sa
from alembic import op

revision = "0009_credential_events"
down_revision = "0008_media_derivations"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "nous_credential_events",
        sa.Column("id", sa.String, primary_key=True),
        sa.Column("credential_id", sa.String, nullable=False),
        sa.Column("revision", sa.Integer, nullable=False),
        sa.Column("commit_seq", sa.Integer, nullable=False),
        sa.Column("record", sa.JSON, nullable=False),
        sa.UniqueConstraint("credential_id", "revision", name="uq_credential_event_revision"),
    )
    op.create_index(
        "ix_nous_credential_events_credential_id", "nous_credential_events", ["credential_id"]
    )


def downgrade() -> None:
    op.drop_table("nous_credential_events")
