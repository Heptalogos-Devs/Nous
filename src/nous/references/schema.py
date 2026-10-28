import sqlalchemy as sa

from nous.platform.persistence import metadata

addresses = sa.Table(
    "reference_addresses",
    metadata,
    sa.Column("address", sa.String, primary_key=True),
    sa.Column("namespace_id", sa.String, nullable=False),
    sa.Column("target", sa.Text, nullable=False),
    sa.Column("commit_seq", sa.Integer, nullable=False),
    sa.UniqueConstraint("namespace_id", "target", name="uq_reference_target"),
)
imported = sa.Table(
    "imported_reference_addresses",
    metadata,
    sa.Column("namespace_id", sa.String, primary_key=True),
    sa.Column("subject_id", sa.String, primary_key=True),
    sa.Column("bundle_id", sa.String, primary_key=True),
    sa.Column("address", sa.String, primary_key=True),
    sa.Column("target", sa.Text, primary_key=True),
    sa.Column("commit_seq", sa.Integer, nullable=False),
)
