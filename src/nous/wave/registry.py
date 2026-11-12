"""Transactional registration and retention of immutable Wave snapshots."""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
from typing import cast
from uuid import uuid4

import sqlalchemy as sa
from pydantic import JsonValue, TypeAdapter
from sqlalchemy.dialects.sqlite import insert
from sqlalchemy.engine import RowMapping

from nous.common.errors import NousError
from nous.common.identifier import new_identifier
from nous.storage.database import WriteTx

from . import schema
from .contracts import (
    BuiltProduct,
    GroupDescriptor,
    SnapshotDescriptor,
    SnapshotOwner,
    SnapshotRef,
)


@dataclass(frozen=True)
class CleanupSelection:
    """Rows removed in the transaction and their unshared persistent assets."""

    removed: list[tuple[str, str]]
    remaining_assets: set[str]


class SnapshotRegistry:
    """Register snapshots and maintain their formal database references."""

    def __init__(self, instance_id: str) -> None:
        self.instance_id = instance_id

    async def publish_in(self, tx: WriteTx, prepared: BuiltProduct) -> SnapshotDescriptor:
        """Publish a complete build and all of its references in one transaction."""
        ref = SnapshotRef(
            instance_id=self.instance_id, id=str(uuid4()), identifier=new_identifier("wav")
        )
        await tx.connection.execute(
            insert(schema.collections)
            .values(id=str(uuid4()), name=prepared.view.name)
            .on_conflict_do_nothing(index_elements=[schema.collections.c.name])
        )
        collection_id = await tx.connection.scalar(
            sa.select(schema.collections.c.id).where(
                schema.collections.c.name == prepared.view.name
            )
        )
        if collection_id is None:
            raise NousError("wave.collection_unavailable", "Cannot create collection")
        graph_layout = TypeAdapter(dict[str, JsonValue]).validate_python(
            prepared.graph_payload["layout"]
        )
        await tx.connection.execute(
            insert(schema.projection_graphs)
            .values(
                signature=prepared.graph_signature,
                concepts=graph_layout["concepts"],
                associations=graph_layout["associations"],
                support_groups=graph_layout["support_groups"],
            )
            .on_conflict_do_nothing(index_elements=[schema.projection_graphs.c.signature])
        )
        graph_artifacts = TypeAdapter(list[str]).validate_python(
            prepared.graph_payload["artifact_ids"]
        )
        if graph_artifacts:
            await tx.connection.execute(
                insert(schema.projection_graph_artifacts).on_conflict_do_nothing(),
                [
                    {"graph_signature": prepared.graph_signature, "artifact_id": ref}
                    for ref in graph_artifacts
                ],
            )
        graph_vectors = TypeAdapter(list[dict[str, JsonValue]]).validate_python(
            prepared.graph_payload["vector_mappings"]
        )
        if graph_vectors:
            await tx.connection.execute(
                insert(schema.projection_graph_vectors).on_conflict_do_nothing(),
                [
                    {
                        "graph_signature": prepared.graph_signature,
                        "key": vector["key"],
                        "input": vector["input"],
                        "result_id": vector["result_id"],
                        "row_index": vector["row_index"],
                    }
                    for vector in graph_vectors
                ],
            )
        await tx.connection.execute(
            schema.snapshots.insert().values(
                id=ref.id,
                identifier=ref.identifier,
                collection_id=collection_id,
                manifest_artifact_id=prepared.view.manifest_artifact.id
                if prepared.view.manifest_artifact
                else None,
                covered_generation=prepared.view.generation,
                profile=prepared.view.profile,
                space_id=prepared.profile.space_id,
                space_definition=prepared.profile.space_definition.model_dump(mode="json")
                if prepared.profile.space_definition
                else None,
                identity=prepared.identity,
                index_location=prepared.index_location,
                layout={
                    key: value
                    for key, value in prepared.layout.items()
                    if key
                    not in {
                        "name",
                        "owner",
                        "metadata",
                        "groups",
                        "concepts",
                        "associations",
                        "support_groups",
                    }
                },
                owner=prepared.view.owner.model_dump(mode="json"),
                metadata=prepared.view.metadata,
                component_signature=prepared.component_signature,
                graph_signature=prepared.graph_signature,
                build_summary=prepared.summary,
                created_at=prepared.created_at,
            )
        )
        declared_groups = {group.key: group for group in prepared.view.groups}
        documents = {document.key: document for document in prepared.view.documents}
        groups: dict[str, GroupDescriptor] = {}
        for unit in prepared.units:
            for candidate in unit.candidates:
                key = cast(str, candidate["group_key"])
                document_key = cast(str, candidate["document"])
                groups.setdefault(
                    key,
                    declared_groups.get(key)
                    or GroupDescriptor(
                        key=key, document_key=document_key, title=documents[document_key].title
                    ),
                )
        if groups:
            await tx.connection.execute(
                schema.snapshot_groups.insert(),
                [
                    {
                        "snapshot_id": ref.id,
                        "key": group.key,
                        "ordinal": ordinal,
                        "document_key": group.document_key,
                        "title": group.title,
                        "context": group.context,
                    }
                    for ordinal, group in enumerate(groups.values())
                ],
            )
        if prepared.units:
            await tx.connection.execute(
                insert(schema.projection_units).on_conflict_do_nothing(
                    index_elements=[schema.projection_units.c.signature]
                ),
                [
                    {
                        "signature": unit.signature,
                        "input_signature": unit.input_signature,
                        "layout_signature": unit.layout_signature,
                        "capture_stamp": unit.capture_stamp,
                        "projection": unit.projection.model_dump(mode="json")
                        if unit.projection is not None
                        else None,
                        "document_key": unit.document_key,
                        "document": unit.document,
                        "roots": unit.roots,
                        "candidates": unit.candidates,
                        "candidate_data": unit.candidate_data,
                    }
                    for unit in prepared.units
                ],
            )
            await tx.connection.execute(
                schema.snapshot_members.insert(),
                [
                    {
                        "snapshot_id": ref.id,
                        "ordinal": ordinal,
                        "unit_signature": unit.signature,
                    }
                    for ordinal, unit in enumerate(prepared.units)
                ],
            )
            candidates = [
                {
                    "snapshot_id": ref.id,
                    "key": candidate["key"],
                    "group_key": candidate["group_key"],
                    "unit_signature": unit.signature,
                    "unit_index": index,
                    "row_index": row,
                }
                for unit in prepared.units
                for index, (candidate, row) in enumerate(
                    zip(unit.candidates, unit.candidate_rows, strict=True)
                )
            ]
            if candidates:
                await tx.connection.execute(schema.snapshot_candidates.insert(), candidates)
            unit_artifacts = [
                {"unit_signature": unit.signature, "artifact_id": artifact_id}
                for unit in prepared.units
                for artifact_id in unit.artifact_ids
            ]
            if unit_artifacts:
                await tx.connection.execute(
                    insert(schema.projection_artifacts).on_conflict_do_nothing(), unit_artifacts
                )
            unit_vectors = [
                {
                    "unit_signature": unit.signature,
                    "key": vector["key"],
                    "input": vector["input"],
                    "result_id": vector["result_id"],
                    "row_index": vector["row_index"],
                }
                for unit in prepared.units
                for vector in unit.vector_mappings
            ]
            if unit_vectors:
                await tx.connection.execute(
                    insert(schema.projection_vectors).on_conflict_do_nothing(), unit_vectors
                )
        artifacts = {ref.id for refs in prepared.view.evidence.roots.values() for ref in refs}
        if artifacts:
            await tx.connection.execute(
                schema.snapshot_artifacts.insert(),
                [
                    {"snapshot_id": ref.id, "artifact_id": artifact_id}
                    for artifact_id in sorted(artifacts)
                ],
            )
        return SnapshotDescriptor(
            ref=ref,
            owner=prepared.view.owner,
            collection=prepared.view.name,
            captured_generation=prepared.view.generation,
            profile_signature=prepared.view.profile,
            space_id=prepared.profile.space_id,
            space_signature=cast(str, prepared.identity["space_signature"]),
            identity=prepared.identity,
            metadata=prepared.view.metadata,
        )

    async def promote_in(self, tx: WriteTx, ref: SnapshotRef) -> None:
        """Point the owning collection to a published snapshot."""
        if ref.instance_id != self.instance_id:
            raise NousError("wave.instance_mismatch", "Snapshot belongs to another instance")
        snapshot = (
            (
                await tx.connection.execute(
                    sa.select(
                        schema.snapshots.c.collection_id, schema.snapshots.c.identifier
                    ).where(schema.snapshots.c.id == ref.id)
                )
            )
            .mappings()
            .one_or_none()
        )
        if snapshot is None:
            raise NousError("wave.snapshot_not_found", "Wave snapshot does not exist")
        if snapshot["identifier"] != ref.identifier:
            raise NousError(
                "wave.snapshot_mismatch", "Snapshot reference differs from its identity"
            )
        await tx.connection.execute(
            schema.collections.update()
            .where(schema.collections.c.id == snapshot["collection_id"])
            .values(current_snapshot_id=ref.id)
        )

    async def retain_in(
        self, tx: WriteTx, ref: SnapshotRef, *, owner_kind: str, owner_id: str
    ) -> None:
        """Persist a formal owner's dependency on an exact snapshot."""
        if ref.instance_id != self.instance_id or not owner_kind or not owner_id:
            raise NousError("wave.invalid_reference", "Invalid snapshot retention reference")
        identifier = await tx.connection.scalar(
            sa.select(schema.snapshots.c.identifier).where(schema.snapshots.c.id == ref.id)
        )
        if identifier != ref.identifier:
            raise NousError("wave.snapshot_not_found", "Snapshot is no longer available")
        await tx.connection.execute(
            insert(schema.snapshot_references)
            .values(snapshot_id=ref.id, owner_kind=owner_kind, owner_id=owner_id)
            .on_conflict_do_nothing()
        )

    async def release_in(self, tx: WriteTx, *, owner_kind: str, owner_id: str) -> None:
        await tx.connection.execute(
            schema.snapshot_references.delete().where(
                schema.snapshot_references.c.owner_kind == owner_kind,
                schema.snapshot_references.c.owner_id == owner_id,
            )
        )

    def descriptor(self, row: RowMapping) -> SnapshotDescriptor:
        return SnapshotDescriptor(
            ref=SnapshotRef(
                instance_id=self.instance_id, id=row["id"], identifier=row["identifier"]
            ),
            owner=TypeAdapter(SnapshotOwner).validate_python(row["owner"]),
            collection=row["name"],
            captured_generation=row["covered_generation"],
            profile_signature=row["profile"],
            space_id=row["space_id"],
            space_signature=row["identity"]["space_signature"],
            identity=row["identity"],
            metadata=row["metadata"],
        )

    async def remove_unreferenced_in(
        self,
        tx: WriteTx,
        *,
        keep_recent: int,
        retain: Sequence[SnapshotRef],
        protected: set[str],
        pending_assets: set[str],
    ) -> CleanupSelection:
        """Delete unreferenced rows while the caller holds the lifecycle lock."""
        removed: list[tuple[str, str]] = []
        rows = (
            (
                await tx.connection.execute(
                    sa.select(
                        schema.snapshots.c.id,
                        schema.snapshots.c.identifier,
                        schema.snapshots.c.index_location,
                        schema.snapshots.c.collection_id,
                        schema.collections.c.current_snapshot_id,
                    )
                    .join(schema.collections)
                    .order_by(schema.snapshots.c.created_at.desc(), schema.snapshots.c.id.desc())
                )
            )
            .mappings()
            .all()
        )
        by_id = {row["id"]: row for row in rows}
        for ref in retain:
            if (
                ref.instance_id != self.instance_id
                or ref.id not in by_id
                or by_id[ref.id]["identifier"] != ref.identifier
            ):
                raise NousError("wave.snapshot_not_found", "Retained snapshot does not exist")
        protected = set(protected) | {ref.id for ref in retain}
        protected.update(row["current_snapshot_id"] for row in rows)
        protected.update(
            (await tx.connection.scalars(sa.select(schema.snapshot_references.c.snapshot_id))).all()
        )
        per_collection: dict[str, int] = {}
        for row in rows:
            count = per_collection.get(row["collection_id"], 0)
            if count < keep_recent:
                protected.add(row["id"])
            per_collection[row["collection_id"]] = count + 1
        obsolete = [row for row in rows if row["id"] not in protected]
        ids = [row["id"] for row in obsolete]
        if ids:
            for table in (
                schema.snapshot_candidates,
                schema.snapshot_groups,
                schema.snapshot_members,
                schema.snapshot_artifacts,
            ):
                await tx.connection.execute(table.delete().where(table.c.snapshot_id.in_(ids)))
            await tx.connection.execute(
                schema.snapshots.delete().where(schema.snapshots.c.id.in_(ids))
            )
            unused = ~sa.exists().where(
                schema.snapshot_members.c.unit_signature == schema.projection_units.c.signature
            )
            unused_units = set(
                (
                    await tx.connection.scalars(
                        sa.select(schema.projection_units.c.signature).where(unused)
                    )
                ).all()
            )
            if unused_units:
                for table in (schema.projection_vectors, schema.projection_artifacts):
                    await tx.connection.execute(
                        table.delete().where(table.c.unit_signature.in_(unused_units))
                    )
            await tx.connection.execute(schema.projection_units.delete().where(unused))
            unused_graph = ~sa.exists().where(
                schema.snapshots.c.graph_signature == schema.projection_graphs.c.signature
            )
            unused_graphs = set(
                (
                    await tx.connection.scalars(
                        sa.select(schema.projection_graphs.c.signature).where(unused_graph)
                    )
                ).all()
            )
            if unused_graphs:
                for table in (
                    schema.projection_graph_vectors,
                    schema.projection_graph_artifacts,
                ):
                    await tx.connection.execute(
                        table.delete().where(table.c.graph_signature.in_(unused_graphs))
                    )
            await tx.connection.execute(schema.projection_graphs.delete().where(unused_graph))
            removed = [(row["id"], row["identifier"]) for row in obsolete]
        remaining_assets = {
            row["index_location"] for row in rows if row["id"] not in ids
        } | pending_assets
        return CleanupSelection(removed, remaining_assets)
