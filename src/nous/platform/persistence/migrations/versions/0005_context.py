"""Persist managed context preparations, admitted cursors and host outcomes."""

import sqlalchemy as sa
from alembic import op

revision = "0005_context"
down_revision = "0004_content_reference_scope"
branch_labels = None
depends_on = None


def upgrade() -> None:
    for name in ("context_preparations", "context_tracks", "context_sessions", "context_reports"):
        columns = [
            sa.Column("id", sa.String(), primary_key=True),
            sa.Column("namespace_id", sa.String(), nullable=False),
            sa.Column("subject_id", sa.String(), nullable=False),
            sa.Column("record", sa.JSON(), nullable=False),
        ]
        if name == "context_preparations":
            columns.append(sa.Column("track_id", sa.String(), nullable=False))
        if name == "context_reports":
            columns.append(sa.Column("admission_id", sa.String(), nullable=False))
        op.create_table(name, *columns)


def downgrade() -> None:
    for name in ("context_reports", "context_sessions", "context_tracks", "context_preparations"):
        op.drop_table(name)
