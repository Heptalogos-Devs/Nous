"""Derived media identities share the existing authority and commit sequence."""

import sqlalchemy as sa

from nous.platform.persistence import metadata

representations = sa.Table(
    "derived_representations",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("revision", sa.Integer, primary_key=True),
    sa.Column("namespace_id", sa.String, nullable=False, index=True),
    sa.Column("subject_id", sa.String, nullable=False, index=True),
    sa.Column("head", sa.Boolean, nullable=False),
    sa.Column("record", sa.JSON, nullable=False),
    sa.Column("commit_seq", sa.Integer, nullable=False),
)
derived_regions = sa.Table(
    "derived_regions",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("namespace_id", sa.String, nullable=False, index=True),
    sa.Column("subject_id", sa.String, nullable=False, index=True),
    sa.Column("record", sa.JSON, nullable=False),
    sa.Column("commit_seq", sa.Integer, nullable=False),
)
