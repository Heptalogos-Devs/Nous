"""Traverse exact coordinate relationships declared by representation producers."""

from __future__ import annotations

import json
from typing import TYPE_CHECKING

import sqlalchemy as sa

from nous.common.errors import NousError

from .contracts import (
    ArtifactRef,
    ContentSelection,
    OriginSlice,
    OutputMapping,
    ResultRef,
    SourceSelection,
)

if TYPE_CHECKING:
    from .artifacts import ArtifactStore


async def map_origins(
    artifacts: ArtifactStore, content: ContentSelection, target: ArtifactRef
) -> tuple[OriginSlice, ...]:
    """Resolve exact mappings iteratively without inferring them from later consumers."""
    await artifacts.get(target)
    root = (await artifacts.normalize_selections((content,)))[0]
    async with artifacts.database.read() as tx:
        rows = (
            (
                await tx.connection.execute(
                    sa.text("""
            WITH RECURSIVE lineage(artifact_id) AS (
                SELECT :source_id
                UNION
                SELECT json_extract(origin.value, '$.artifact.id')
                FROM lineage
                JOIN processing_outputs AS output ON output.artifact_id = lineage.artifact_id
                JOIN content_output_mappings AS mapping
                  ON mapping.result_id = output.result_id AND mapping.output_name = output.name
                JOIN json_each(mapping.origins) AS origin
                WHERE lineage.artifact_id != :target_id AND mapping.relation != 'derived'
            )
            SELECT output.artifact_id, result.id AS transform_id, result.identifier,
                   mapping.output_name, mapping.selection, mapping.relation, mapping.origins
            FROM lineage
            JOIN processing_outputs AS output ON output.artifact_id = lineage.artifact_id
            JOIN processing_results AS result ON result.id = output.result_id
            JOIN content_output_mappings AS mapping
              ON mapping.result_id = output.result_id AND mapping.output_name = output.name
            WHERE mapping.relation != 'derived'
            ORDER BY output.artifact_id, mapping.ordinal
        """),
                    {"source_id": root.artifact.id, "target_id": target.id},
                )
            )
            .mappings()
            .all()
        )
    raw_mappings = [
        OutputMapping(
            output_name=str(row["output_name"]),
            selection=SourceSelection.model_validate(json.loads(row["selection"])),
            relation=row["relation"],
            origins=tuple(
                ContentSelection.model_validate(item) for item in json.loads(row["origins"])
            ),
        )
        for row in rows
    ]
    normalized_origins = await artifacts.normalize_selections(
        [origin for mapping in raw_mappings for origin in mapping.origins]
    )
    mapped: dict[str, tuple[ResultRef, list[OutputMapping]]] = {}
    offset = 0
    for row, mapping in zip(rows, raw_mappings, strict=True):
        count = len(mapping.origins)
        mapping = mapping.model_copy(
            update={"origins": normalized_origins[offset : offset + count]}
        )
        offset += count
        item = mapped.setdefault(
            str(row["artifact_id"]),
            (
                ResultRef(
                    id=str(row["transform_id"]),
                    identifier=str(row["identifier"]),
                    instance_id=artifacts.instance_id,
                ),
                [],
            ),
        )
        item[1].append(mapping)
    pending: list[tuple[ContentSelection, bool]] = [(root, False)]
    active: set[str] = set()
    done: dict[str, tuple[OriginSlice, ...]] = {}
    children: dict[str, tuple[ResultRef, tuple[ContentSelection, ...]]] = {}

    while pending:
        current, expanded = pending.pop()
        key = current.model_dump_json()
        if key in done:
            continue
        if expanded:
            transform, origins = children.pop(key)
            answer = tuple(
                dict.fromkeys(
                    OriginSlice(origin=item.origin, transform_ref=transform)
                    for origin in origins
                    for item in done[origin.model_dump_json()]
                )
            )
            active.remove(key)
            done[key] = answer
            continue
        if current.artifact == target:
            done[key] = (OriginSlice(origin=current),)
            continue
        if key in active:
            raise NousError(
                "content.origin_unmapped", "Representation coordinate mapping contains a cycle"
            )
        start, end = current.selection.start_byte, current.selection.end_byte
        if start is None or end is None:
            raise NousError(
                "content.origin_unmapped", "Representation requires exact byte coordinates"
            )
        candidate = mapped.get(current.artifact.id)
        mappings = list(candidate[1]) if candidate else []
        mappings.sort(key=lambda item: item.selection.start_byte or 0)
        cursor = start
        selected_origins: list[ContentSelection] = []
        for mapping in mappings:
            low, high = mapping.selection.start_byte, mapping.selection.end_byte
            assert low is not None and high is not None
            if high <= cursor or low >= end:
                continue
            left, right = max(cursor, low), min(end, high)
            if low > cursor or right <= left:
                break
            if mapping.relation == "identity":
                origin = mapping.origins[0]
                origin_start = origin.selection.start_byte
                assert origin_start is not None
                selected_origins.append(
                    ContentSelection(
                        artifact=origin.artifact,
                        selection=SourceSelection(
                            start_byte=origin_start + left - low,
                            end_byte=origin_start + right - low,
                        ),
                    )
                )
            elif mapping.relation == "atomic":
                selected_origins.extend(mapping.origins)
            else:
                break
            cursor = right
        if cursor != end or not selected_origins or candidate is None:
            raise NousError(
                "content.origin_unmapped",
                "Representation has no exact mapping to the requested body",
                details={
                    "content": current.model_dump(mode="json"),
                    "target": target.model_dump(mode="json"),
                },
            )
        normalized = tuple(selected_origins)
        transform = candidate[0]
        active.add(key)
        children[key] = (transform, normalized)
        pending.append((current, True))
        pending.extend((origin, False) for origin in reversed(normalized))

    return done[root.model_dump_json()]
