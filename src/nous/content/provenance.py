"""Related-ancestor queries over normalized content relations."""

from __future__ import annotations

from collections.abc import Sequence
from typing import TYPE_CHECKING

import sqlalchemy as sa
from pydantic import BaseModel, ConfigDict

from nous.common.errors import NousError
from nous.storage.database import Database

from . import schema
from .contracts import ArtifactRef

if TYPE_CHECKING:
    from .artifacts import ArtifactStore


class ProvenanceRelation(BaseModel):
    model_config = ConfigDict(frozen=True)
    child: ArtifactRef
    parent: ArtifactRef
    result_id: str
    role: str


class ProvenanceSummary(BaseModel):
    model_config = ConfigDict(frozen=True)
    terminal_roots: tuple[ArtifactRef, ...]
    support_roots: tuple[ArtifactRef, ...] = ()
    traversed_results: tuple[str, ...] = ()
    relations: tuple[ProvenanceRelation, ...] = ()
    complete: bool = True


class ProvenanceReader:
    """SQLite traverses only requested ancestors; shared DAG nodes are visited once per target."""

    def __init__(self, database: Database, artifacts: ArtifactStore) -> None:
        self.database, self.artifacts = database, artifacts

    async def read(
        self, refs: Sequence[str | ArtifactRef], *, details: bool = False
    ) -> dict[str, ProvenanceSummary]:
        if not refs:
            return {}
        targets: list[str] = []
        for ref in refs:
            if isinstance(ref, ArtifactRef):
                if ref.instance_id != self.artifacts.instance_id:
                    raise NousError(
                        "content.instance_mismatch", "Artifact belongs to another instance"
                    )
                targets.append(ref.id)
            else:
                targets.append(ref)
        async with self.database.read() as tx:
            rows = (
                (
                    await tx.connection.execute(
                        sa.select(schema.artifacts.c.id, schema.artifacts.c.identifier).where(
                            sa.or_(
                                schema.artifacts.c.id.in_(targets),
                                schema.artifacts.c.identifier.in_(targets),
                            )
                        )
                    )
                )
                .mappings()
                .all()
            )
            resolved = {str(row["id"]): str(row["id"]) for row in rows}
            resolved.update({str(row["identifier"]): str(row["id"]) for row in rows})
            missing = [target for target in targets if target not in resolved]
            if missing:
                raise NousError(
                    "content.not_found",
                    "Provenance target does not exist",
                    details={"refs": missing},
                )
            object_ids = list(dict.fromkeys(resolved[target] for target in targets))
            walk = (
                sa.select(schema.artifacts.c.id.label("root"), schema.artifacts.c.id.label("node"))
                .where(schema.artifacts.c.id.in_(object_ids))
                .cte("ancestors", recursive=True)
            )

            def input_step(table: sa.Table):
                return sa.select(walk.c.root, table.c.artifact_id.label("node")).select_from(
                    walk.join(schema.outputs, schema.outputs.c.artifact_id == walk.c.node).join(
                        table, table.c.result_id == schema.outputs.c.result_id
                    )
                )

            part_step = input_step(schema.parts)
            origin_step = input_step(schema.origins)
            material_step = sa.select(
                walk.c.root, schema.material_source_targets.c.artifact_id.label("node")
            ).select_from(
                walk.join(schema.outputs, schema.outputs.c.artifact_id == walk.c.node)
                .join(
                    schema.material_source_outputs,
                    sa.and_(
                        schema.material_source_outputs.c.result_id == schema.outputs.c.result_id,
                        schema.material_source_outputs.c.output_name == schema.outputs.c.name,
                    ),
                )
                .join(
                    schema.material_source_targets,
                    sa.and_(
                        schema.material_source_targets.c.result_id
                        == schema.material_source_outputs.c.result_id,
                        schema.material_source_targets.c.source_ordinal
                        == schema.material_source_outputs.c.source_ordinal,
                    ),
                )
            )
            walk = walk.union(part_step, origin_step, material_step)
            nodes = (
                (
                    await tx.connection.execute(
                        sa.select(
                            walk.c.root, walk.c.node, schema.artifacts.c.identifier
                        ).select_from(
                            walk.outerjoin(schema.artifacts, schema.artifacts.c.id == walk.c.node)
                        )
                    )
                )
                .mappings()
                .all()
            )
            reachable = sa.select(walk.c.node).distinct().subquery()

            def edge_select(table: sa.Table):
                return sa.select(
                    schema.outputs.c.artifact_id.label("child"),
                    table.c.artifact_id.label("parent"),
                    schema.outputs.c.result_id,
                    schema.inputs.c.role,
                ).select_from(
                    reachable.join(schema.outputs, schema.outputs.c.artifact_id == reachable.c.node)
                    .join(table, table.c.result_id == schema.outputs.c.result_id)
                    .join(
                        schema.inputs,
                        sa.and_(
                            schema.inputs.c.result_id == table.c.result_id,
                            schema.inputs.c.group_ordinal == table.c.group_ordinal,
                        ),
                    )
                )

            material_edges = sa.select(
                schema.outputs.c.artifact_id.label("child"),
                schema.material_source_targets.c.artifact_id.label("parent"),
                schema.outputs.c.result_id,
                schema.material_sources.c.kind.label("role"),
            ).select_from(
                reachable.join(schema.outputs, schema.outputs.c.artifact_id == reachable.c.node)
                .join(
                    schema.material_source_outputs,
                    sa.and_(
                        schema.material_source_outputs.c.result_id == schema.outputs.c.result_id,
                        schema.material_source_outputs.c.output_name == schema.outputs.c.name,
                    ),
                )
                .join(
                    schema.material_sources,
                    sa.and_(
                        schema.material_sources.c.result_id
                        == schema.material_source_outputs.c.result_id,
                        schema.material_sources.c.ordinal
                        == schema.material_source_outputs.c.source_ordinal,
                    ),
                )
                .join(
                    schema.material_source_targets,
                    sa.and_(
                        schema.material_source_targets.c.result_id
                        == schema.material_source_outputs.c.result_id,
                        schema.material_source_targets.c.source_ordinal
                        == schema.material_source_outputs.c.source_ordinal,
                    ),
                )
            )
            edges = (
                (
                    await tx.connection.execute(
                        sa.union(
                            edge_select(schema.parts), edge_select(schema.origins), material_edges
                        )
                    )
                )
                .mappings()
                .all()
            )
        identities: dict[str, ArtifactRef] = {}
        per_target: dict[str, set[str]] = {object_id: set() for object_id in object_ids}
        for row in nodes:
            if row["identifier"] is None:
                raise NousError("content.provenance_missing", "A provenance ancestor is missing")
            object_id = str(row["node"])
            identities[object_id] = ArtifactRef(
                id=object_id,
                identifier=str(row["identifier"]),
                instance_id=self.artifacts.instance_id,
            )
            per_target[str(row["root"])].add(object_id)
        parents: dict[str, set[str]] = {}
        supporting: dict[str, set[tuple[str, str]]] = {}
        for edge in edges:
            parents.setdefault(str(edge["child"]), set()).add(str(edge["parent"]))
            if edge["role"] in {
                "source",
                "reference",
                "current_basis",
                "context",
                "revision_base",
                "edit_target",
            }:
                supporting.setdefault(str(edge["child"]), set()).add(
                    (str(edge["parent"]), edge["role"])
                )
        memo: dict[str, set[str]] = {}
        support_memo: dict[str, set[str]] = {}
        for object_id in object_ids:
            active: set[str] = set()
            pending = [(object_id, False)]
            while pending:
                node, expanded = pending.pop()
                if node in memo:
                    continue
                if expanded:
                    memo[node] = (
                        set().union(*(memo[parent] for parent in parents.get(node, ())))
                        if parents.get(node)
                        else {node}
                    )
                    support_memo[node] = (
                        set().union(
                            *(
                                support_memo[parent]
                                for parent, role in supporting.get(node, ())
                                if role in {"source", "reference", "current_basis"}
                                or parent in parents
                            )
                        )
                        if parents.get(node)
                        else {node}
                    )
                    active.remove(node)
                    continue
                if node in active:
                    raise NousError(
                        "content.provenance_cycle", "Content provenance contains a cycle"
                    )
                active.add(node)
                pending.append((node, True))
                pending.extend(
                    (parent, False) for parent in parents.get(node, ()) if parent not in memo
                )
        output: dict[str, ProvenanceSummary] = {}
        for object_id in object_ids:
            related = (
                [edge for edge in edges if str(edge["child"]) in per_target[object_id]]
                if details
                else []
            )
            output[object_id] = ProvenanceSummary(
                terminal_roots=tuple(identities[root] for root in sorted(memo[object_id])),
                support_roots=tuple(identities[root] for root in sorted(support_memo[object_id])),
                traversed_results=tuple(sorted({str(edge["result_id"]) for edge in related})),
                relations=tuple(
                    ProvenanceRelation(
                        child=identities[str(edge["child"])],
                        parent=identities[str(edge["parent"])],
                        result_id=str(edge["result_id"]),
                        role=edge["role"],
                    )
                    for edge in related
                ),
            )
        return output
