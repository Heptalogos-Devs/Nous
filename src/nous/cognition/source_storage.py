"""Persist exact sources for immutable written revisions and drafts."""

from collections.abc import Sequence
from typing import TYPE_CHECKING

import sqlalchemy as sa
from pydantic import TypeAdapter
from sqlalchemy.engine import RowMapping

from nous.common.material_refs import RecordRevisionRef, WorkRevisionRef
from nous.content.contracts import (
    ArtifactRef,
    ContentSelection,
    MaterialRef,
    MaterialSource,
    ResultOutputRef,
    SourceSelection,
)
from nous.storage.database import ReadTx, WriteTx

from . import schema

if TYPE_CHECKING:
    from nous.content.artifacts import ArtifactStore

_MATERIAL_REF = TypeAdapter(MaterialRef)


async def save_sources_in(
    tx: WriteTx, owner_id: str, sources: Sequence[MaterialSource], *, draft: bool = False
) -> None:
    table = schema.draft_sources if draft else schema.sources
    targets = schema.draft_source_targets if draft else schema.source_targets
    owner = "draft_id" if draft else "revision_id"
    if not sources:
        return
    await tx.connection.execute(
        table.insert(),
        [
            {
                owner: owner_id,
                "key": source.key,
                "ordinal": ordinal,
                "material": source.material.model_dump(mode="json"),
                "target_revision_id": source.material.revision_id
                if isinstance(source.material, RecordRevisionRef)
                else None,
                "source_result_id": source.material.result.id
                if isinstance(source.material, ResultOutputRef)
                else None,
                "source_work_revision_id": source.material.revision_id
                if isinstance(source.material, WorkRevisionRef)
                else None,
                "usage": source.kind,
                "name": source.name,
            }
            for ordinal, source in enumerate(sources)
        ],
    )
    await tx.connection.execute(
        targets.insert(),
        [
            {
                owner: owner_id,
                "key": source.key,
                "ordinal": ordinal,
                "artifact_id": selection.artifact.id,
                "selection": selection.selection.model_dump(mode="json"),
            }
            for source in sources
            for ordinal, selection in enumerate(source.selections)
        ],
    )


async def read_sources_in(
    tx: ReadTx, artifacts: ArtifactStore, owner_ids: Sequence[str], *, draft: bool = False
) -> dict[str, tuple[MaterialSource, ...]]:
    table = schema.draft_sources if draft else schema.sources
    targets = schema.draft_source_targets if draft else schema.source_targets
    owner = "draft_id" if draft else "revision_id"
    grouped: dict[tuple[str, str], tuple[RowMapping, list[ContentSelection]]] = {}
    ids = list(dict.fromkeys(owner_ids))
    for start in range(0, len(ids), 800):
        rows = (
            await tx.connection.execute(
                sa.select(
                    table,
                    targets.c.selection,
                    schema.artifacts.c.id.label("artifact_id"),
                    schema.artifacts.c.identifier.label("artifact_identifier"),
                )
                .select_from(
                    table.join(
                        targets,
                        sa.and_(table.c[owner] == targets.c[owner], table.c.key == targets.c.key),
                    ).join(schema.artifacts, schema.artifacts.c.id == targets.c.artifact_id)
                )
                .where(table.c[owner].in_(ids[start : start + 800]))
                .order_by(table.c[owner], table.c.ordinal, targets.c.ordinal)
            )
        ).mappings()
        for row in rows:
            key = (str(row[owner]), str(row["key"]))
            _, selections = grouped.setdefault(key, (row, []))
            selections.append(
                ContentSelection(
                    artifact=ArtifactRef(
                        id=str(row["artifact_id"]),
                        identifier=str(row["artifact_identifier"]),
                        instance_id=artifacts.instance_id,
                    ),
                    selection=SourceSelection.model_validate(row["selection"]),
                )
            )
    output: dict[str, list[MaterialSource]] = {owner_id: [] for owner_id in owner_ids}
    for (owner_id, key), (row, selections) in grouped.items():
        output[owner_id].append(
            MaterialSource(
                key=key,
                material=_MATERIAL_REF.validate_python(row["material"]),
                selections=tuple(selections),
                kind=row["usage"],
                name=row["name"],
            )
        )
    return {key: tuple(value) for key, value in output.items()}
