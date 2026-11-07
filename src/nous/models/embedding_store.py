"""Persistent embedding-space identity and its adopted vector results."""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass

import sqlalchemy as sa
from sqlalchemy.dialects.sqlite import insert

from nous.common.errors import NousError
from nous.configuration import ConfigSnapshot
from nous.storage.database import Database, ReadTx, WriteTx

from .embedding_schema import spaces, vector_rows, vectors
from .embeddings import resolved_space
from .vector_contracts import SpaceDefinition


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
            for space_id in snapshot.values.embedding.spaces:
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
    ) -> dict[str, StoredVectorRow]:
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
                insert(vector_rows).on_conflict_do_nothing(),
                [
                    {
                        "space_id": definition.space_id,
                        "purpose": purpose,
                        "result_id": result_id,
                        "row_index": index,
                        "input_digest": digest,
                    }
                    for index, digest in enumerate(input_digests)
                ],
            )

        return await self.find_rows_in(tx, definition, purpose, input_digests)

    async def find_rows_in(
        self, tx: ReadTx, definition: SpaceDefinition, purpose: str, digests: Sequence[str]
    ) -> dict[str, StoredVectorRow]:
        found: dict[str, StoredVectorRow] = {}
        await self.check_in(tx, definition)
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
                    vector_rows.c.space_id == definition.space_id,
                    vector_rows.c.purpose == purpose,
                    vector_rows.c.input_digest.in_(digests[start : start + 900]),
                )
            )
            for row in (await tx.connection.execute(statement)).mappings():
                found[str(row["input_digest"])] = StoredVectorRow(
                    result_id=str(row["result_id"]),
                    artifact_id=str(row["artifact_id"]),
                    row_index=int(row["row_index"]),
                )
        return found

    async def find_rows(
        self, definition: SpaceDefinition, purpose: str, digests: Sequence[str]
    ) -> dict[str, StoredVectorRow]:
        async with self._database.read() as tx:
            return await self.find_rows_in(tx, definition, purpose, digests)

    async def resolve_row(
        self, definition: SpaceDefinition, purpose: str, artifact_id: str, row_index: int
    ) -> StoredVectorRow:
        async with self._database.read() as tx:
            stored = await tx.connection.scalar(
                sa.select(spaces.c.definition).where(spaces.c.space_id == definition.space_id)
            )
            if stored != definition.model_dump(mode="json"):
                raise NousError(
                    "embedding.space_mismatch", "Vector reference does not match its adopted space"
                )
            row = (
                (
                    await tx.connection.execute(
                        sa.select(vectors).where(
                            vectors.c.artifact_id == artifact_id,
                            vectors.c.space_id == definition.space_id,
                            vectors.c.purpose == purpose,
                        )
                    )
                )
                .mappings()
                .one_or_none()
            )
            if row is None or not 0 <= row_index < int(row["input_count"]):
                raise NousError(
                    "embedding.vector_not_found",
                    "Vector reference does not identify an adopted row",
                )
            return StoredVectorRow(str(row["result_id"]), artifact_id, row_index)

    async def status(self) -> dict[str, object]:
        async with self._database.read() as tx:
            definitions = (await tx.connection.execute(sa.select(spaces))).mappings().all()
            counts = (
                (
                    await tx.connection.execute(
                        sa.select(
                            vectors.c.space_id, sa.func.sum(vectors.c.input_count).label("rows")
                        ).group_by(vectors.c.space_id)
                    )
                )
                .mappings()
                .all()
            )
        return {
            "spaces": [dict(row) for row in definitions],
            "adopted_rows": {str(row["space_id"]): int(row["rows"]) for row in counts},
        }
