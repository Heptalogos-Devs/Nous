"""Persist readable addresses and backfill existing canonical identities.

Revision ID: 0007_reference_directory
Revises: 0006_credentials
"""

import secrets

import sqlalchemy as sa
from alembic import op
from nous_wave.contracts import CognitiveRef, MediaReference
from nous_wave.contracts.references import SemanticTarget
from sqlalchemy.dialects.sqlite import insert

from nous.platform.persistence.authority import canonical_json
from nous.references.allocation import prefix, targets
from nous.references.codec import encode
from nous.references.schema import addresses

revision = "0007_reference_directory"
down_revision = "0006_credentials"
branch_labels = None
depends_on = None


def _ensure(connection: sa.Connection, namespace_id: str, target: SemanticTarget, seq: int) -> str:
    encoded = canonical_json(target)
    existing = connection.scalar(
        sa.select(addresses.c.address).where(
            addresses.c.namespace_id == namespace_id, addresses.c.target == encoded
        )
    )
    if existing is not None:
        return str(existing)
    for _ in range(32):
        candidate = prefix(target) + ":" + encode(secrets.randbits(48))
        result = connection.execute(
            insert(addresses)
            .values(address=candidate, namespace_id=namespace_id, target=encoded, commit_seq=seq)
            .on_conflict_do_nothing()
        )
        if result.rowcount == 1:
            return candidate
    raise RuntimeError("Readable reference allocation exhausted collision bound")


def upgrade() -> None:
    op.create_table(
        "reference_addresses",
        sa.Column("address", sa.String, primary_key=True),
        sa.Column("namespace_id", sa.String, nullable=False),
        sa.Column("target", sa.Text, nullable=False),
        sa.Column("commit_seq", sa.Integer, nullable=False),
        sa.UniqueConstraint("namespace_id", "target", name="uq_reference_target"),
    )
    op.create_table(
        "imported_reference_addresses",
        sa.Column("namespace_id", sa.String, primary_key=True),
        sa.Column("subject_id", sa.String, primary_key=True),
        sa.Column("bundle_id", sa.String, primary_key=True),
        sa.Column("address", sa.String, primary_key=True),
        sa.Column("target", sa.Text, primary_key=True),
        sa.Column("commit_seq", sa.Integer, nullable=False),
    )
    connection = op.get_bind()
    cognitive = sa.table(
        "cognitive_revisions",
        sa.column("id", sa.String),
        sa.column("revision", sa.Integer),
        sa.column("namespace_id", sa.String),
        sa.column("record", sa.JSON),
        sa.column("commit_seq", sa.Integer),
    )
    for row in connection.execute(sa.select(cognitive)).mappings():
        record = dict(row["record"])
        reference = CognitiveRef.model_validate(record["reference"])
        allocated = [
            _ensure(connection, row["namespace_id"], target, row["commit_seq"])
            for target in targets(reference)
        ]
        record["addresses"] = {"object": allocated[0], "exact": allocated[1]}
        connection.execute(
            cognitive.update()
            .where(cognitive.c.id == row["id"], cognitive.c.revision == row["revision"])
            .values(record=record)
        )
    for table_name, kind in (("materials", "material"), ("source_regions", "source_region")):
        table = sa.table(
            table_name, sa.column("id", sa.String), sa.column("namespace_id", sa.String)
        )
        for row in connection.execute(sa.select(table)).mappings():
            reference = MediaReference.model_validate({"kind": kind, "id": row["id"]})
            for target in targets(reference):
                _ensure(connection, row["namespace_id"], target, 0)


def downgrade() -> None:
    op.drop_table("imported_reference_addresses")
    op.drop_table("reference_addresses")
