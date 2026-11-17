import sqlalchemy as sa

metadata = sa.MetaData()
blobs = sa.Table(
    "content_blobs",
    metadata,
    sa.Column("digest", sa.String, primary_key=True),
    sa.Column("byte_length", sa.Integer, nullable=False),
)
references = sa.Table(
    "content_references",
    metadata,
    sa.Column("owner_ref", sa.String, primary_key=True),
    sa.Column("slot", sa.String, primary_key=True),
    sa.Column("digest", sa.String, sa.ForeignKey("content_blobs.digest"), nullable=False),
)
