"""Exact coordinate maps declared by adopted representation producers."""

from __future__ import annotations

from typing import TYPE_CHECKING, Literal, cast

import sqlalchemy as sa
from pydantic import Field, JsonValue

from nous.common.errors import NousError

from . import schema
from .contracts import (
    ArtifactRef,
    ContentSelection,
    ContentValue,
    OriginSlice,
    ResultRef,
    SourceSelection,
)

if TYPE_CHECKING:
    from .artifacts import ArtifactStore


class TextOriginMapping(ContentValue):
    start_byte: int = Field(ge=0)
    end_byte: int = Field(ge=0)
    origins: tuple[ContentSelection, ...]
    mode: Literal["identity", "atomic", "whole"] = "atomic"
    output_name: str | None = None


async def map_origins(
    artifacts: ArtifactStore,
    content: ContentSelection,
    target: ArtifactRef,
    trail: frozenset[str] = frozenset(),
) -> tuple[OriginSlice, ...]:
    """Follow only published coordinate mappings, retaining discontinuous source regions."""
    content = (await artifacts.normalize_selections((content,)))[0]
    await artifacts.get(target)
    if content.artifact == target:
        return (OriginSlice(origin=content),)
    identity = content.model_dump_json()
    if identity in trail:
        raise NousError(
            "content.origin_unmapped", "Representation coordinate mapping contains a cycle"
        )
    trail = trail | {identity}
    selected = content.selection
    if selected.start_byte is None or selected.end_byte is None:
        raise NousError(
            "content.origin_unmapped", "Representation requires an explicit text coordinate mapping"
        )
    await artifacts.get(content.artifact)
    alternatives: list[tuple[ResultRef | None, tuple[TextOriginMapping, ...]]] = []
    async with artifacts.database.read() as tx:
        rows = (
            (
                await tx.connection.execute(
                    sa.select(
                        schema.results.c.id,
                        schema.results.c.identifier,
                        schema.results.c.coverage,
                        schema.outputs.c.name,
                    )
                    .join(schema.outputs)
                    .where(schema.outputs.c.artifact_id == content.artifact.id)
                )
            )
            .mappings()
            .all()
        )
        count_rows = (
            (
                await tx.connection.execute(
                    sa.select(schema.outputs.c.result_id, sa.func.count())
                    .where(schema.outputs.c.result_id.in_([row["id"] for row in rows]))
                    .group_by(schema.outputs.c.result_id)
                )
            ).all()
            if rows
            else ()
        )
        counts = {str(item[0]): int(item[1]) for item in count_rows}
    blob = await artifacts.blob(content.artifact)
    for row in rows:
        coverage = cast(dict[str, JsonValue], row["coverage"])
        declared = coverage.get("text_mappings")
        if not isinstance(declared, list):
            continue
        try:
            mappings = tuple(TextOriginMapping.model_validate(item) for item in declared)
        except ValueError as error:
            raise NousError(
                "content.origin_unmapped", "Adopted representation mapping is invalid"
            ) from error
        mappings = tuple(
            item
            for item in mappings
            if item.output_name == row["name"]
            or item.output_name is None
            and counts.get(row["id"]) == 1
        )
        if any(not 0 <= item.start_byte <= item.end_byte <= blob.byte_length for item in mappings):
            raise NousError(
                "content.origin_unmapped", "Representation mapping exceeds its accepted output"
            )
        alternatives.append(
            (
                ResultRef(
                    id=str(row["id"]),
                    identifier=str(row["identifier"]),
                    instance_id=artifacts.instance_id,
                ),
                mappings,
            )
        )
    if not alternatives:
        alternatives = await _input_origin_maps(artifacts, content)
    answers: list[tuple[OriginSlice, ...]] = []
    for transform, mappings in alternatives:
        try:
            exact = next(
                (
                    item
                    for item in mappings
                    if item.mode == "whole"
                    and (item.start_byte, item.end_byte) == (selected.start_byte, selected.end_byte)
                ),
                None,
            )
            segments = (
                (exact,)
                if exact
                else tuple(
                    sorted(
                        (
                            item
                            for item in mappings
                            if item.mode != "whole"
                            and item.end_byte > selected.start_byte
                            and item.start_byte < selected.end_byte
                        ),
                        key=lambda item: (item.start_byte, item.end_byte),
                    )
                )
            )
            cursor = selected.start_byte
            result: list[OriginSlice] = []
            for mapping in segments:
                left, right = (
                    max(cursor, mapping.start_byte),
                    min(selected.end_byte, mapping.end_byte),
                )
                if mapping.start_byte > cursor or right <= left:
                    raise NousError(
                        "content.origin_unmapped", "Selected representation has unmapped bytes"
                    )
                origins = await artifacts.normalize_selections(mapping.origins)
                if mapping.mode == "identity":
                    if len(origins) != 1:
                        raise NousError(
                            "content.origin_unmapped", "Identity mapping requires one source range"
                        )
                    origin = origins[0]
                    start, end = origin.selection.start_byte, origin.selection.end_byte
                    if (
                        start is None
                        or end is None
                        or end - start != mapping.end_byte - mapping.start_byte
                    ):
                        raise NousError(
                            "content.origin_unmapped",
                            "Identity mapping has incompatible source coordinates",
                        )
                    origins = (
                        ContentSelection(
                            artifact=origin.artifact,
                            selection=SourceSelection(
                                start_byte=start + left - mapping.start_byte,
                                end_byte=start + right - mapping.start_byte,
                            ),
                        ),
                    )
                elif left != mapping.start_byte or right != mapping.end_byte:
                    raise NousError(
                        "content.origin_unmapped",
                        "Selection cuts an indivisible representation mapping",
                    )
                for origin in origins:
                    resolved = await map_origins(artifacts, origin, target, trail)
                    result.extend(
                        OriginSlice(
                            origin=item.origin, transform_ref=transform or item.transform_ref
                        )
                        for item in resolved
                    )
                cursor = right
            if cursor != selected.end_byte or not result:
                raise NousError(
                    "content.origin_unmapped", "Selected representation has no exact source mapping"
                )
            answer = tuple(dict.fromkeys(result))
            if answer not in answers:
                answers.append(answer)
        except NousError as error:
            if error.code != "content.origin_unmapped":
                raise
    if len(answers) != 1:
        raise NousError(
            "content.origin_unmapped",
            "Representation has no unique exact mapping to the requested body",
            details={
                "content": content.model_dump(mode="json"),
                "target": target.model_dump(mode="json"),
            },
        )
    return answers[0]


async def _input_origin_maps(
    artifacts: ArtifactStore, content: ContentSelection
) -> list[tuple[ResultRef | None, tuple[TextOriginMapping, ...]]]:
    """Use explicitly adopted InputPart/OriginSlice relations as atomic maps."""
    async with artifacts.database.read() as tx:
        rows = (
            (
                await tx.connection.execute(
                    sa.select(
                        schema.parts,
                        schema.origins.c.origin_ordinal,
                        schema.origins.c.artifact_id.label("origin_artifact"),
                        schema.origins.c.selector.label("origin_selector"),
                        schema.origins.c.transform_result_id,
                        schema.results.c.identifier.label("transform_identifier"),
                    )
                    .join(
                        schema.origins,
                        sa.and_(
                            schema.parts.c.result_id == schema.origins.c.result_id,
                            schema.parts.c.group_ordinal == schema.origins.c.group_ordinal,
                            schema.parts.c.part_ordinal == schema.origins.c.part_ordinal,
                        ),
                    )
                    .outerjoin(
                        schema.results, schema.results.c.id == schema.origins.c.transform_result_id
                    )
                    .where(schema.parts.c.artifact_id == content.artifact.id)
                    .order_by(
                        schema.parts.c.result_id,
                        schema.parts.c.group_ordinal,
                        schema.parts.c.part_ordinal,
                        schema.origins.c.origin_ordinal,
                    )
                )
            )
            .mappings()
            .all()
        )
        originals = await artifacts.get_many_in(tx, [str(row["origin_artifact"]) for row in rows])
    grouped: dict[tuple[str, int, int], list[tuple[ContentSelection, ResultRef | None]]] = {}
    selectors: dict[tuple[str, int, int], SourceSelection] = {}
    for row, original in zip(rows, originals, strict=True):
        key = (str(row["result_id"]), int(row["group_ordinal"]), int(row["part_ordinal"]))
        selectors[key] = SourceSelection.model_validate(row["selector"])
        transform = (
            ResultRef(
                id=str(row["transform_result_id"]),
                identifier=str(row["transform_identifier"]),
                instance_id=artifacts.instance_id,
            )
            if row["transform_result_id"] is not None
            else None
        )
        grouped.setdefault(key, []).append(
            (
                ContentSelection(
                    artifact=original.ref,
                    selection=SourceSelection.model_validate(row["origin_selector"]),
                ),
                transform,
            )
        )
    alternatives: list[tuple[ResultRef | None, tuple[TextOriginMapping, ...]]] = []
    for key, origins in grouped.items():
        selected = (
            await artifacts.normalize_selections(
                (ContentSelection(artifact=content.artifact, selection=selectors[key]),)
            )
        )[0].selection
        if selected.start_byte is None or selected.end_byte is None:
            continue
        if any(item[1] != origins[0][1] for item in origins):
            continue
        alternatives.append(
            (
                origins[0][1],
                (
                    TextOriginMapping(
                        start_byte=selected.start_byte,
                        end_byte=selected.end_byte,
                        origins=tuple(item[0] for item in origins),
                    ),
                ),
            )
        )
    return alternatives
