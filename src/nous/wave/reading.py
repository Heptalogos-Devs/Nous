"""Read immutable snapshot identities, candidate units and mappings."""

from __future__ import annotations

import json
from collections.abc import Sequence
from typing import cast

import numpy as np
import numpy.typing as npt
import sqlalchemy as sa
from sqlalchemy.engine import RowMapping

from nous.common.errors import NousError
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ContentSelection
from nous.storage.database import Database

from . import schema
from .contracts import (
    GroupDescriptor,
    SnapshotDescriptor,
    SnapshotRef,
    WaveNeighborhoodResult,
    WaveReadResult,
)
from .mapping import CandidateMapping, SnapshotRow
from .registry import SnapshotRegistry


class SnapshotReader:
    """Resolve immutable snapshot metadata and bounded candidate lookups."""

    def __init__(self, database: Database, instance_id: str, registry: SnapshotRegistry) -> None:
        self.database, self.instance_id, self.registry = database, instance_id, registry

    async def lookup(
        self, ref: str | SnapshotRef, *, include_mapping: bool = False, current: bool = False
    ) -> RowMapping | None:
        exact = ref if isinstance(ref, SnapshotRef) else None
        if exact is not None:
            if exact.instance_id != self.instance_id:
                raise NousError("wave.instance_mismatch", "Snapshot belongs to another instance")
            ref = exact.id
        columns = (
            list(schema.snapshots.c)
            if include_mapping
            else [
                schema.snapshots.c.id,
                schema.snapshots.c.identifier,
                schema.snapshots.c.owner,
                schema.snapshots.c.metadata,
                schema.snapshots.c.covered_generation,
                schema.snapshots.c.profile,
                schema.snapshots.c.space_id,
                schema.snapshots.c.identity,
            ]
        )
        query = sa.select(*columns, schema.collections.c.name).join(
            schema.collections, schema.snapshots.c.collection_id == schema.collections.c.id
        )
        condition = (
            sa.and_(
                schema.collections.c.name == ref,
                schema.collections.c.current_snapshot_id == schema.snapshots.c.id,
            )
            if current
            else sa.or_(schema.snapshots.c.id == ref, schema.snapshots.c.identifier == ref)
        )
        async with self.database.read() as tx:
            row = (await tx.connection.execute(query.where(condition))).mappings().one_or_none()
            if row is None and exact is None and not current:
                row = (
                    (
                        await tx.connection.execute(
                            query.where(
                                schema.collections.c.name == ref,
                                schema.collections.c.current_snapshot_id == schema.snapshots.c.id,
                            )
                        )
                    )
                    .mappings()
                    .one_or_none()
                )
        if (
            row is not None
            and exact is not None
            and (row["id"] != exact.id or row["identifier"] != exact.identifier)
        ):
            raise NousError(
                "wave.snapshot_mismatch", "Snapshot reference differs from its identity"
            )
        return row

    async def current(self, collection: str) -> SnapshotDescriptor | None:
        row = await self.lookup(collection, current=True)
        return self.registry.descriptor(row) if row is not None else None

    async def describe(self, ref: str | SnapshotRef) -> SnapshotDescriptor:
        """Read only immutable identity, ownership and summary columns."""
        row = await self.lookup(ref)
        if row is None:
            raise NousError(
                "wave.snapshot_not_found", "Wave snapshot does not exist", details={"ref": str(ref)}
            )
        return self.registry.descriptor(row)

    async def candidates(
        self, ref: SnapshotRef, keys: Sequence[str]
    ) -> dict[str, CandidateMapping]:
        """Load only units containing the requested candidate keys, in bounded batches."""
        result: dict[str, CandidateMapping] = {}
        unique = list(dict.fromkeys(keys))
        async with self.database.read() as tx:
            positions: list[tuple[str, str, int]] = []
            for offset in range(0, len(unique), 400):
                rows = (
                    await tx.connection.execute(
                        sa.select(
                            schema.snapshot_candidates.c.key,
                            schema.snapshot_candidates.c.unit_signature,
                            schema.snapshot_candidates.c.unit_index,
                        ).where(
                            schema.snapshot_candidates.c.snapshot_id == ref.id,
                            schema.snapshot_candidates.c.key.in_(unique[offset : offset + 400]),
                        )
                    )
                ).mappings()
                for row in rows:
                    positions.append((row["key"], row["unit_signature"], row["unit_index"]))
            signatures = list({signature for _, signature, _ in positions})
            units: dict[str, list[CandidateMapping]] = {}
            for offset in range(0, len(signatures), 400):
                rows = (
                    await tx.connection.execute(
                        sa.select(
                            schema.projection_units.c.signature,
                            schema.projection_units.c.candidates,
                        ).where(
                            schema.projection_units.c.signature.in_(
                                signatures[offset : offset + 400]
                            )
                        )
                    )
                ).mappings()
                for row in rows:
                    units[row["signature"]] = row["candidates"]
            for key, signature, index in positions:
                result[key] = units[signature][index]
        return result

    async def groups(self, ref: SnapshotRef, keys: Sequence[str]) -> dict[str, GroupDescriptor]:
        """Read only the requested immutable group contexts."""
        result: dict[str, GroupDescriptor] = {}
        unique = list(dict.fromkeys(keys))
        async with self.database.read() as tx:
            for offset in range(0, len(unique), 400):
                rows = (
                    await tx.connection.execute(
                        sa.select(schema.snapshot_groups).where(
                            schema.snapshot_groups.c.snapshot_id == ref.id,
                            schema.snapshot_groups.c.key.in_(unique[offset : offset + 400]),
                        )
                    )
                ).mappings()
                for row in rows:
                    result[row["key"]] = GroupDescriptor(
                        key=row["key"],
                        document_key=row["document_key"],
                        title=row["title"],
                        context=row["context"],
                    )
        return result

    async def resolve(self, ref: str | SnapshotRef) -> SnapshotRow:
        row = await self.lookup(ref, include_mapping=True)
        if row is None:
            raise NousError(
                "wave.snapshot_not_found", "Wave snapshot does not exist", details={"ref": str(ref)}
            )
        async with self.database.read() as tx:
            members = (
                (
                    await tx.connection.execute(
                        sa.select(
                            schema.projection_units.c.signature,
                            schema.projection_units.c.document_key,
                            schema.projection_units.c.document,
                            schema.projection_units.c.roots,
                            schema.projection_units.c.candidates,
                        )
                        .select_from(schema.snapshot_members.join(schema.projection_units))
                        .where(schema.snapshot_members.c.snapshot_id == row["id"])
                        .order_by(schema.snapshot_members.c.ordinal)
                    )
                )
                .mappings()
                .all()
            )
            positions = (
                (
                    await tx.connection.execute(
                        sa.select(
                            schema.snapshot_candidates.c.unit_signature,
                            schema.snapshot_candidates.c.unit_index,
                            schema.snapshot_candidates.c.row_index,
                        )
                        .where(schema.snapshot_candidates.c.snapshot_id == row["id"])
                        .order_by(schema.snapshot_candidates.c.row_index)
                    )
                )
                .mappings()
                .all()
            )
            graph = (
                (
                    await tx.connection.execute(
                        sa.select(
                            schema.projection_graphs.c.concepts,
                            schema.projection_graphs.c.associations,
                            schema.projection_graphs.c.support_groups,
                        ).where(schema.projection_graphs.c.signature == row["graph_signature"])
                    )
                )
                .mappings()
                .one()
            )
            groups = (
                (
                    await tx.connection.execute(
                        sa.select(schema.snapshot_groups)
                        .where(schema.snapshot_groups.c.snapshot_id == row["id"])
                        .order_by(schema.snapshot_groups.c.ordinal)
                    )
                )
                .mappings()
                .all()
            )
        documents = {member["document_key"]: member["document"] for member in members}
        units = {member["signature"]: member["candidates"] for member in members}
        roots = {key: refs for member in members for key, refs in member["roots"].items()}
        candidates = [
            units[position["unit_signature"]][position["unit_index"]] for position in positions
        ]
        value = dict(row)
        layout = dict(value.pop("layout"))
        roots = layout.pop("roots", roots)
        value["mapping"] = {
            **layout,
            "name": value["name"],
            "owner": value["owner"],
            "metadata": value["metadata"],
            "concepts": graph["concepts"],
            "associations": graph["associations"],
            "support_groups": graph["support_groups"],
            "roots": roots,
            "documents": documents,
            "groups": {
                group["key"]: {
                    "key": group["key"],
                    "document_key": group["document_key"],
                    "title": group["title"],
                    "context": group["context"],
                }
                for group in groups
            },
            "candidates": candidates,
        }
        return cast(SnapshotRow, value)


async def read_candidate(
    row: SnapshotRow,
    candidate_key: str,
    artifacts: ArtifactStore,
    instance_id: str,
    *,
    sources: bool = False,
) -> WaveReadResult:
    """Read the exact source and optional support facts of a snapshot candidate."""
    mapping = row["mapping"]
    candidate = next((item for item in mapping["candidates"] if item["key"] == candidate_key), None)
    if candidate is None:
        raise NousError(
            "wave.candidate_not_found",
            "Candidate does not exist in this snapshot",
            details={"candidate": candidate_key},
        )
    content = ContentSelection.model_validate(candidate["content"])
    return WaveReadResult.model_validate(
        {
            "snapshot": {
                "instance_id": instance_id,
                "id": row["id"],
                "identifier": row["identifier"],
            },
            "candidate": {"snapshot": row["identifier"], "key": candidate_key},
            "content": content,
            "origin_contents": candidate["origin_contents"],
            "text": await artifacts.read_selection(content),
            "group": mapping["groups"][candidate["group_key"]],
            "concepts": candidate["concepts"],
            "paths": candidate["paths"],
            "sources": [
                {"evidence_root": root, "artifacts": mapping["roots"][root]}
                for root in candidate["roots"]
            ]
            if sources
            else None,
        }
    )


def neighborhood_rows(row: SnapshotRow, concepts: Sequence[str]) -> npt.NDArray[np.uint64]:
    """Bind requested concept identities to their rows in this snapshot."""
    rows = {item["key"]: index for index, item in enumerate(row["mapping"]["concepts"])}
    unknown = set(concepts) - rows.keys()
    if unknown:
        raise NousError(
            "wave.concept_not_found",
            "Neighborhood refers to unknown concept keys",
            details={"concepts": sorted(unknown)},
        )
    return np.asarray([rows[key] for key in concepts], dtype=np.uint64)


def describe_neighborhood(
    row: SnapshotRow, concepts: Sequence[str], result: str, instance_id: str
) -> WaveNeighborhoodResult:
    """Translate snapshot-local neighborhood rows into formal identities."""
    all_concepts = row["mapping"]["concepts"]
    neighborhood = json.loads(result)
    for edge in neighborhood.get("edges", []):
        edge["source"] = all_concepts[edge["source"]]["key"]
        edge["target"] = all_concepts[edge["target"]]["key"]
    for node in neighborhood.get("nodes", []):
        node["concept"] = all_concepts[node.pop("row")]["key"]
        node["roots"] = [row["mapping"]["root_order"][index] for index in node.pop("root_rows", [])]
        node["source_witness_groups"] = [
            [row["mapping"]["root_order"][index] for index in group]
            for group in node.get("source_witness_groups", [])
        ]
        node["candidates"] = [
            row["mapping"]["candidates"][index]["key"] for index in node.pop("candidate_rows", [])
        ]
    for association in neighborhood.get("associations", []):
        association["source"] = all_concepts[association["source"]]["key"]
        association["target"] = all_concepts[association["target"]]["key"]
        group = association.get("support_group")
        if isinstance(group, dict):
            group = cast(dict[str, object], group)
            association["support_group"] = next(
                (
                    item
                    for item in row["mapping"]["support_groups"]
                    if item["key"] == group.get("key")
                ),
                group,
            )
        association["evidence_roots"] = [
            row["mapping"]["root_order"][index] for index in association["evidence_roots"]
        ]
    return WaveNeighborhoodResult.model_validate(
        {
            "snapshot": {
                "instance_id": instance_id,
                "id": row["id"],
                "identifier": row["identifier"],
            },
            "concepts": concepts,
            "neighborhood": neighborhood,
        }
    )
