"""Persistent embedding-space identity and its adopted vector results."""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass

import sqlalchemy as sa

from nous.common.errors import NousError
from nous.configuration import ConfigSnapshot
from nous.content.schema import metadata
from nous.storage.database import Database, ReadTx, WriteTx

from .embeddings import SpaceDefinition, resolved_space

spaces = sa.Table(
    "embedding_spaces",
    metadata,
    sa.Column("space_id", sa.String, primary_key=True),
    sa.Column("definition", sa.JSON, nullable=False),
)
vectors = sa.Table(
    "embedding_vectors",
    metadata,
    sa.Column("result_id", sa.String, sa.ForeignKey("processing_results.id"), primary_key=True),
    sa.Column("space_id", sa.String, sa.ForeignKey("embedding_spaces.space_id"), nullable=False),
    sa.Column("artifact_id", sa.String, sa.ForeignKey("content_artifacts.id"), nullable=False),
    sa.Column("purpose", sa.String, nullable=False),
    sa.Column("input_count", sa.Integer, nullable=False),
)

vector_rows = sa.Table(
    "embedding_vector_rows",
    metadata,
    sa.Column("id", sa.Integer, primary_key=True, autoincrement=True),
    sa.Column("result_id", sa.String, sa.ForeignKey("embedding_vectors.result_id"), nullable=False),
    sa.Column("row_index", sa.Integer, nullable=False),
    sa.Column("input_digest", sa.String(64), nullable=False),
    sa.UniqueConstraint("result_id", "row_index"),
)
sa.Index("ix_embedding_vector_rows_digest", vector_rows.c.input_digest)


@dataclass(frozen=True)
class StoredVectorRow:
    """An adopted matrix and the exact row produced for one encoded input."""

    result_id: str
    artifact_id: str
    row_index: int


class EmbeddingSpaceRegistry:
    def __init__(self, database: Database) -> None:
        self._database = database

    async def check_in(self, tx: ReadTx, definition: SpaceDefinition) -> None:
        existing = await tx.connection.scalar(
            sa.select(spaces.c.definition).where(spaces.c.space_id == definition.space_id)
        )
        if existing is not None and existing != definition.model_dump(mode="json"):
            raise NousError(
                "configuration.binding_conflict",
                "An embedding space with stored vectors cannot change its semantic definition; use a new space ID.",
                details={"space_id": definition.space_id},
            )

    async def check(self, definition: SpaceDefinition) -> None:
        async with self._database.read() as tx:
            await self.check_in(tx, definition)

    async def check_snapshot(self, snapshot: ConfigSnapshot) -> None:
        async with self._database.read() as tx:
            for space_id in snapshot.typed_values().embedding.spaces:
                await self.check_in(tx, resolved_space(snapshot, space_id))

    async def bind_in(self, tx: WriteTx, definition: SpaceDefinition) -> None:
        await self.check_in(tx, definition)
        existing = await tx.connection.scalar(
            sa.select(spaces.c.space_id).where(spaces.c.space_id == definition.space_id)
        )
        if existing is None:
            await tx.connection.execute(
                spaces.insert().values(
                    space_id=definition.space_id,
                    definition=definition.model_dump(mode="json"),
                )
            )

    async def attach_in(
        self,
        tx: WriteTx,
        *,
        definition: SpaceDefinition,
        result_id: str,
        artifact_id: str,
        purpose: str,
        input_digests: Sequence[str],
    ) -> None:
        await tx.connection.execute(
            vectors.insert().values(
                result_id=result_id,
                space_id=definition.space_id,
                artifact_id=artifact_id,
                purpose=purpose,
                input_count=len(input_digests),
            )
        )
        if input_digests:
            await tx.connection.execute(
                vector_rows.insert(),
                [
                    {"result_id": result_id, "row_index": index, "input_digest": digest}
                    for index, digest in enumerate(input_digests)
                ],
            )

    async def find_rows(
        self, definition: SpaceDefinition, purpose: str, digests: Sequence[str]
    ) -> dict[str, StoredVectorRow]:
        """Return the first adopted exact row; space semantics are checked before reuse."""
        found: dict[str, StoredVectorRow] = {}
        async with self._database.read() as tx:
            await self.check_in(tx, definition)
            # Stay below SQLite's host-parameter limit even for large collections.
            for start in range(0, len(digests), 900):
                statement = (
                    sa.select(
                        vector_rows.c.input_digest,
                        vector_rows.c.result_id,
                        vectors.c.artifact_id,
                        vector_rows.c.row_index,
                    )
                    .join(vectors, vectors.c.result_id == vector_rows.c.result_id)
                    .where(
                        vectors.c.space_id == definition.space_id,
                        vectors.c.purpose == purpose,
                        vector_rows.c.input_digest.in_(digests[start : start + 900]),
                    )
                    .order_by(vector_rows.c.id)
                )
                for row in (await tx.connection.execute(statement)).mappings():
                    digest = str(row["input_digest"])
                    found.setdefault(
                        digest,
                        StoredVectorRow(
                            result_id=str(row["result_id"]),
                            artifact_id=str(row["artifact_id"]),
                            row_index=int(row["row_index"]),
                        ),
                    )
        return found
