"""Batch-load completed result facts and exact provenance references."""

from __future__ import annotations

from collections.abc import Sequence
from datetime import datetime
from typing import TYPE_CHECKING, cast

import sqlalchemy as sa
from pydantic import JsonValue, TypeAdapter
from sqlalchemy.engine import RowMapping

from nous.common.errors import NousError
from nous.storage.database import ReadTx

from . import schema
from ._records import date_value
from .contracts import (
    ArtifactRef,
    ContentSelection,
    InputGroup,
    InputPart,
    NamedSelection,
    OriginSlice,
    OutputMapping,
    ProcessingResult,
    ProducerFacts,
    ResultOutput,
    ResultOutputRef,
    ResultRef,
    SelectedInputs,
    SelectedSource,
    SourceContext,
    SourceFacts,
    SourceSelection,
)

if TYPE_CHECKING:
    from .artifacts import ArtifactStore

_JSON = TypeAdapter(dict[str, JsonValue])
_PRODUCER = TypeAdapter(ProducerFacts)


async def outputs_for_artifacts_in(
    tx: ReadTx, artifacts: ArtifactStore, refs: Sequence[ArtifactRef]
) -> dict[str, ResultOutputRef]:
    """Find the producing named output of each exact artifact in this read view."""
    values = await artifacts.get_many_in(tx, refs)
    identifiers = list(dict.fromkeys(item.ref.id for item in values))
    output: dict[str, ResultOutputRef] = {}
    for offset in range(0, len(identifiers), 800):
        rows = (
            await tx.connection.execute(
                sa.select(
                    schema.outputs.c.artifact_id,
                    schema.outputs.c.name,
                    schema.results.c.id,
                    schema.results.c.identifier,
                )
                .join(schema.results, schema.results.c.id == schema.outputs.c.result_id)
                .where(schema.outputs.c.artifact_id.in_(identifiers[offset : offset + 800]))
            )
        ).mappings()
        for row in rows:
            output[str(row["artifact_id"])] = ResultOutputRef(
                result=ResultRef(
                    id=str(row["id"]),
                    identifier=str(row["identifier"]),
                    instance_id=artifacts.instance_id,
                ),
                output=str(row["name"]),
            )
    return output


async def get_many_in(
    tx: ReadTx, artifacts: ArtifactStore, refs: Sequence[str | ResultRef]
) -> tuple[ProcessingResult, ...]:
    """Read complete result facts in request order using the caller's read view."""
    targets: list[str] = []
    for ref in refs:
        if isinstance(ref, ResultRef):
            if ref.instance_id != artifacts.instance_id:
                raise NousError("content.instance_mismatch", "Result belongs to another instance")
            targets.append(ref.id)
        else:
            targets.append(ref)
    if not targets:
        return ()
    unique = list(dict.fromkeys(targets))
    found: dict[str, RowMapping] = {}
    for start in range(0, len(unique), 400):
        chunk = unique[start : start + 400]
        rows = (
            await tx.connection.execute(
                sa.select(schema.results).where(
                    sa.or_(schema.results.c.id.in_(chunk), schema.results.c.identifier.in_(chunk))
                )
            )
        ).mappings()
        for row in rows:
            found[str(row["id"])] = found[str(row["identifier"])] = row
    for ref, target in zip(refs, targets, strict=True):
        row = found.get(target)
        if row is None:
            raise NousError(
                "content.not_found", "Processing result does not exist", details={"ref": target}
            )
        if isinstance(ref, ResultRef) and ref.identifier != row["identifier"]:
            raise NousError(
                "content.reference_mismatch", "Result reference identity does not match"
            )
    result_ids = list(dict.fromkeys(str(found[target]["id"]) for target in targets))

    async def rows_for(table: sa.Table, order: tuple[str, ...]) -> dict[str, list[RowMapping]]:
        grouped: dict[str, list[RowMapping]] = {}
        for start in range(0, len(result_ids), 800):
            rows = (
                await tx.connection.execute(
                    sa.select(table)
                    .where(table.c.result_id.in_(result_ids[start : start + 800]))
                    .order_by(table.c.result_id, *(table.c[key] for key in order))
                )
            ).mappings()
            for row in rows:
                grouped.setdefault(str(row["result_id"]), []).append(row)
        return grouped

    selected = await rows_for(schema.selected_sources, ("ordinal",))
    targets_by_result = await rows_for(schema.selected_targets, ("slot_key", "ordinal"))
    inputs_by_result = await rows_for(schema.inputs, ("group_ordinal",))
    parts_by_result = await rows_for(schema.parts, ("group_ordinal", "part_ordinal"))
    origins_by_result = await rows_for(
        schema.origins, ("group_ordinal", "part_ordinal", "origin_ordinal")
    )
    outputs_by_result = await rows_for(schema.outputs, ("ordinal",))
    mappings_by_result = await rows_for(schema.output_mappings, ("output_name", "ordinal"))
    mapping_origins_by_result = await rows_for(
        schema.mapping_origins, ("output_name", "mapping_ordinal", "ordinal")
    )
    selections_by_result = await rows_for(schema.named_selections, ("ordinal",))
    dependencies_by_result = await rows_for(schema.dependencies, ("ordinal",))
    artifact_ids = {
        str(row["artifact_id"])
        for collection in (
            targets_by_result,
            parts_by_result,
            origins_by_result,
            outputs_by_result,
            mapping_origins_by_result,
            dependencies_by_result,
        )
        for rows in collection.values()
        for row in rows
        if row["artifact_id"] is not None
    }
    identities = {
        value.ref.id: value.ref for value in await artifacts.get_many_in(tx, list(artifact_ids))
    }
    linked_ids = {
        str(row[column])
        for collection, column in (
            (origins_by_result, "transform_result_id"),
            (dependencies_by_result, "dependency_result_id"),
        )
        for rows in collection.values()
        for row in rows
        if row[column] is not None
    }
    linked: dict[str, ResultRef] = {}
    linked_list = list(linked_ids)
    for start in range(0, len(linked_list), 800):
        rows = (
            await tx.connection.execute(
                sa.select(schema.results.c.id, schema.results.c.identifier).where(
                    schema.results.c.id.in_(linked_list[start : start + 800])
                )
            )
        ).mappings()
        linked.update(
            {
                str(row["id"]): ResultRef(
                    id=str(row["id"]),
                    identifier=str(row["identifier"]),
                    instance_id=artifacts.instance_id,
                )
                for row in rows
            }
        )
    if linked_ids - linked.keys():
        raise NousError("content.provenance_missing", "A linked result is missing")
    assembled: dict[str, ProcessingResult] = {}
    for result_id in result_ids:
        row = found[result_id]
        origins: dict[tuple[int, int], list[OriginSlice]] = {}
        for source in origins_by_result.get(result_id, ()):
            key = (int(source["group_ordinal"]), int(source["part_ordinal"]))
            origins.setdefault(key, []).append(
                OriginSlice(
                    origin=ContentSelection(
                        artifact=identities[str(source["artifact_id"])],
                        selection=SourceSelection.model_validate(source["selector"]),
                    ),
                    transform_ref=linked.get(str(source["transform_result_id"])),
                    precision=source["precision"],
                )
            )
        grouped_parts: dict[int, list[InputPart]] = {}
        for source in parts_by_result.get(result_id, ()):
            ordinal = int(source["group_ordinal"])
            grouped_parts.setdefault(ordinal, []).append(
                InputPart(
                    content=ContentSelection(
                        artifact=identities[str(source["artifact_id"])],
                        selection=SourceSelection.model_validate(source["selector"]),
                    ),
                    representation=str(source["representation"]),
                    media_type=source["media_type"],
                    source_media_type=source["source_media_type"],
                    origins=tuple(origins.get((ordinal, int(source["part_ordinal"])), ())),
                )
            )
        inputs = tuple(
            InputGroup(
                key=str(source["key"]),
                slot_key=str(source["slot_key"]),
                role=source["role"],
                name=str(source["name"]),
                context=SourceContext.model_validate(source["context"])
                if source["context"]
                else None,
                parts=tuple(grouped_parts[int(source["group_ordinal"])]),
            )
            for source in inputs_by_result.get(result_id, ())
        )
        outputs = tuple(
            ResultOutput(
                name=str(item["name"]),
                artifact=identities[str(item["artifact_id"])],
                role=item["role"],
                representation_kind=item["representation_kind"],
            )
            for item in outputs_by_result.get(result_id, ())
        )
        by_slot: dict[str, list[ContentSelection]] = {}
        for item in targets_by_result.get(result_id, ()):
            by_slot.setdefault(str(item["slot_key"]), []).append(
                ContentSelection(
                    artifact=identities[str(item["artifact_id"])],
                    selection=SourceSelection.model_validate(item["selector"]),
                )
            )
        selected_inputs = SelectedInputs(
            sources=tuple(
                SelectedSource(
                    slot_key=str(item["slot_key"]),
                    role=item["role"],
                    display_name=item["name"],
                    selections=tuple(by_slot[str(item["slot_key"])]),
                )
                for item in selected.get(result_id, ())
            ),
            facts=tuple(SourceFacts.model_validate(item) for item in row["selected_facts"]),
        )
        mapping_sources: dict[tuple[str, int], list[ContentSelection]] = {}
        for item in mapping_origins_by_result.get(result_id, ()):
            mapping_sources.setdefault(
                (str(item["output_name"]), int(item["mapping_ordinal"])), []
            ).append(
                ContentSelection(
                    artifact=identities[str(item["artifact_id"])],
                    selection=SourceSelection.model_validate(item["selector"]),
                )
            )
        mappings = tuple(
            OutputMapping(
                output_name=str(item["output_name"]),
                selection=SourceSelection.model_validate(item["selection"]),
                relation=item["relation"],
                origins=tuple(mapping_sources[(str(item["output_name"]), int(item["ordinal"]))]),
            )
            for item in mappings_by_result.get(result_id, ())
        )
        selections = tuple(
            NamedSelection(
                key=str(item["key"]),
                sources=tuple(ContentSelection.model_validate(value) for value in item["sources"]),
            )
            for item in selections_by_result.get(result_id, ())
        )
        dependencies: list[ArtifactRef | ResultRef] = []
        for item in dependencies_by_result.get(result_id, ()):
            if item["artifact_id"] is not None:
                dependencies.append(identities[str(item["artifact_id"])])
            else:
                dependencies.append(linked[str(item["dependency_result_id"])])
        assembled[result_id] = ProcessingResult(
            ref=ResultRef(
                id=result_id,
                identifier=str(row["identifier"]),
                instance_id=artifacts.instance_id,
            ),
            operation=str(row["operation"]),
            selected_inputs=selected_inputs,
            inputs=inputs,
            outputs=outputs,
            mappings=mappings,
            selections=selections,
            dependencies=tuple(dependencies),
            producer=_PRODUCER.validate_python(row["producer"]),
            request=_JSON.validate_python(row["request"]),
            parameters=_JSON.validate_python(row["parameters"]),
            coverage=_JSON.validate_python(row["coverage"]),
            formed_at=cast(datetime, date_value(row["formed_at"])),
            recorded_at=cast(datetime, date_value(row["recorded_at"])),
        )
    return tuple(assembled[str(found[target]["id"])] for target in targets)
