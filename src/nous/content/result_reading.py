"""Read result identities and their stored content relationships in batches."""

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
    MaterialSource,
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
    SourceRole,
    SourceSelection,
)

if TYPE_CHECKING:
    from .artifacts import ArtifactStore

_JSON = TypeAdapter(dict[str, JsonValue])
_PRODUCER = TypeAdapter(ProducerFacts)
_SOURCE_ROLE = TypeAdapter(SourceRole)


async def _rows(
    tx: ReadTx, table: sa.Table, ids: Sequence[str], order: tuple[str, ...]
) -> dict[str, list[RowMapping]]:
    grouped: dict[str, list[RowMapping]] = {}
    for start in range(0, len(ids), 800):
        values = (
            await tx.connection.execute(
                sa.select(table)
                .where(table.c.result_id.in_(ids[start : start + 800]))
                .order_by(table.c.result_id, *(table.c[key] for key in order))
            )
        ).mappings()
        for row in values:
            grouped.setdefault(str(row["result_id"]), []).append(row)
    return grouped


async def _result_rows(
    tx: ReadTx, artifacts: ArtifactStore, refs: Sequence[str | ResultRef], *, full: bool = False
) -> tuple[RowMapping, ...]:
    targets = [ref.id if isinstance(ref, ResultRef) else ref for ref in refs]
    if any(isinstance(ref, ResultRef) and ref.instance_id != artifacts.instance_id for ref in refs):
        raise NousError("content.instance_mismatch", "Result belongs to another instance")
    found: dict[str, RowMapping] = {}
    unique = list(dict.fromkeys(targets))
    for start in range(0, len(unique), 400):
        chunk = unique[start : start + 400]
        statement = (
            sa.select(schema.results)
            if full
            else sa.select(schema.results.c.id, schema.results.c.identifier)
        )
        rows = (
            await tx.connection.execute(
                statement.where(
                    sa.or_(schema.results.c.id.in_(chunk), schema.results.c.identifier.in_(chunk))
                )
            )
        ).mappings()
        for row in rows:
            found[str(row["id"])] = found[str(row["identifier"])] = row
    result: list[RowMapping] = []
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
        result.append(row)
    return tuple(result)


def _ref(row: RowMapping, instance_id: str) -> ResultRef:
    return ResultRef(id=str(row["id"]), identifier=str(row["identifier"]), instance_id=instance_id)


async def refs_in(
    tx: ReadTx, artifacts: ArtifactStore, refs: Sequence[str | ResultRef]
) -> tuple[ResultRef, ...]:
    return tuple(
        _ref(row, artifacts.instance_id) for row in await _result_rows(tx, artifacts, refs)
    )


async def outputs_in(
    tx: ReadTx, artifacts: ArtifactStore, requests: Sequence[tuple[str | ResultRef, str | None]]
) -> tuple[tuple[ResultOutputRef, ArtifactRef], ...]:
    """Resolve exact named outputs after resolving their result identities."""
    refs = await refs_in(tx, artifacts, tuple(ref for ref, _ in requests))
    rows = await _rows(
        tx, schema.outputs, tuple(dict.fromkeys(ref.id for ref in refs)), ("ordinal",)
    )
    chosen: list[RowMapping] = []
    for ref, (_, name) in zip(refs, requests, strict=True):
        values = rows.get(ref.id, ())
        if name is None:
            choices = [item for item in values if item["name"] == "evidence"] or [
                item for item in values if item["role"] == "primary"
            ]
            if len(choices) != 1:
                raise NousError("material.output_required", "Choose a named result output")
            chosen.append(choices[0])
        else:
            item = next((item for item in values if item["name"] == name), None)
            if item is None:
                raise NousError(
                    "content.not_found",
                    "Result output does not exist",
                    details={"result": ref.identifier, "output": name},
                )
            chosen.append(item)
    values = await artifacts.get_many_in(tx, [str(row["artifact_id"]) for row in chosen])
    return tuple(
        (ResultOutputRef(result=ref, output=str(row["name"])), artifact.ref)
        for ref, row, artifact in zip(refs, chosen, values, strict=True)
    )


async def _identities_in(
    tx: ReadTx, artifacts: ArtifactStore, *collections: dict[str, list[RowMapping]]
) -> dict[str, ArtifactRef]:
    ids = list(
        dict.fromkeys(
            str(row["artifact_id"])
            for collection in collections
            for rows in collection.values()
            for row in rows
            if row["artifact_id"] is not None
        )
    )
    return {item.ref.id: item.ref for item in await artifacts.get_many_in(tx, ids)}


def _content(row: RowMapping, identities: dict[str, ArtifactRef]) -> ContentSelection:
    return ContentSelection(
        artifact=identities[str(row["artifact_id"])],
        selection=SourceSelection.model_validate(row["selector"]),
    )


async def _sources_in(
    tx: ReadTx,
    artifacts: ArtifactStore,
    ids: Sequence[str],
    targets: dict[str, list[RowMapping]] | None = None,
    identities: dict[str, ArtifactRef] | None = None,
    source_rows: dict[str, list[RowMapping]] | None = None,
    links: dict[str, list[RowMapping]] | None = None,
) -> tuple[
    dict[str, tuple[MaterialSource, ...]], dict[tuple[str, str], tuple[MaterialSource, ...]]
]:
    if source_rows is None:
        source_rows = await _rows(tx, schema.material_sources, ids, ("ordinal",))
    if targets is None:
        targets = await _rows(
            tx, schema.material_source_targets, ids, ("source_ordinal", "ordinal")
        )
    if links is None:
        links = await _rows(tx, schema.material_source_outputs, ids, ("output_name", "ordinal"))
    if identities is None:
        identities = await _identities_in(tx, artifacts, targets)
    result: dict[str, tuple[MaterialSource, ...]] = {}
    outputs: dict[tuple[str, str], tuple[MaterialSource, ...]] = {}
    for result_id in ids:
        by_source: dict[int, list[ContentSelection]] = {}
        for row in targets.get(result_id, ()):
            by_source.setdefault(int(row["source_ordinal"]), []).append(_content(row, identities))
        indexed = {
            int(row["ordinal"]): MaterialSource.model_validate(
                {
                    **{key: row[key] for key in ("key", "kind", "name", "material")},
                    "selections": by_source[int(row["ordinal"])],
                }
            )
            for row in source_rows.get(result_id, ())
        }
        result[result_id] = tuple(indexed.values())
        groups: dict[str, list[MaterialSource]] = {}
        for row in links.get(result_id, ()):
            groups.setdefault(str(row["output_name"]), []).append(
                indexed[int(row["source_ordinal"])]
            )
        outputs.update({(result_id, name): tuple(values) for name, values in groups.items()})
    return result, outputs


async def _mappings_in(
    tx: ReadTx,
    artifacts: ArtifactStore,
    ids: Sequence[str],
    origins: dict[str, list[RowMapping]] | None = None,
    identities: dict[str, ArtifactRef] | None = None,
) -> dict[str, tuple[OutputMapping, ...]]:
    mappings = await _rows(tx, schema.output_mappings, ids, ("output_name", "ordinal"))
    if origins is None:
        origins = await _rows(
            tx, schema.mapping_origins, ids, ("output_name", "mapping_ordinal", "ordinal")
        )
    if identities is None:
        identities = await _identities_in(tx, artifacts, origins)
    result: dict[str, tuple[OutputMapping, ...]] = {}
    for result_id in ids:
        grouped: dict[tuple[str, int], list[ContentSelection]] = {}
        for row in origins.get(result_id, ()):
            grouped.setdefault((str(row["output_name"]), int(row["mapping_ordinal"])), []).append(
                _content(row, identities)
            )
        result[result_id] = tuple(
            OutputMapping(
                output_name=str(row["output_name"]),
                selection=SourceSelection.model_validate(row["selection"]),
                relation=row["relation"],
                origins=tuple(grouped[(str(row["output_name"]), int(row["ordinal"]))]),
            )
            for row in mappings.get(result_id, ())
        )
    return result


async def source_facts_in(
    tx: ReadTx,
    artifacts: ArtifactStore,
    ref: str | ResultRef,
    output: str,
    *,
    additional_kinds: tuple[str, ...] = (),
) -> tuple[
    ResultRef,
    str,
    dict[str, JsonValue],
    tuple[MaterialSource, ...],
    tuple[MaterialSource, ...] | None,
]:
    """Read one result header and its formal source associations without input hydration."""
    row = (await _result_rows(tx, artifacts, (ref,), full=True))[0]
    result = _ref(row, artifacts.instance_id)
    ids = (result.id,)
    output_rows = await _rows(tx, schema.outputs, ids, ("ordinal",))
    output_links = await _rows(tx, schema.material_source_outputs, ids, ("output_name", "ordinal"))
    links = {
        result.id: [
            value for value in output_links.get(result.id, ()) if value["output_name"] == output
        ]
    }
    ordinals = [int(value["source_ordinal"]) for value in links[result.id]]
    source_rows = (
        (
            await tx.connection.execute(
                sa.select(schema.material_sources)
                .where(schema.material_sources.c.result_id == result.id)
                .where(
                    sa.or_(
                        schema.material_sources.c.ordinal.in_(ordinals),
                        schema.material_sources.c.kind.in_(additional_kinds),
                    )
                )
                .order_by(schema.material_sources.c.ordinal)
            )
        )
        .mappings()
        .all()
    )
    chosen = [int(value["ordinal"]) for value in source_rows]
    target_rows = (
        (
            await tx.connection.execute(
                sa.select(schema.material_source_targets)
                .where(
                    schema.material_source_targets.c.result_id == result.id,
                    schema.material_source_targets.c.source_ordinal.in_(chosen),
                )
                .order_by(
                    schema.material_source_targets.c.source_ordinal,
                    schema.material_source_targets.c.ordinal,
                )
            )
        )
        .mappings()
        .all()
    )
    sources, by_output = await _sources_in(
        tx,
        artifacts,
        ids,
        targets={result.id: list(target_rows)},
        source_rows={result.id: list(source_rows)},
        links=links,
    )
    selected = (
        by_output.get((result.id, output), ())
        if any(value["name"] == output for value in output_rows.get(result.id, ()))
        else None
    )
    return (
        result,
        str(row["operation"]),
        _JSON.validate_python(row["request"]),
        sources[result.id],
        selected,
    )


async def output_sources_in(
    tx: ReadTx, artifacts: ArtifactStore, refs: Sequence[ResultOutputRef]
) -> dict[tuple[str, str], tuple[MaterialSource, ...]]:
    ids = tuple(dict.fromkeys(ref.result.id for ref in refs))
    _, values = await _sources_in(tx, artifacts, ids)
    return {
        (ref.result.id, ref.output): values.get((ref.result.id, ref.output), ()) for ref in refs
    }


async def output_mappings_in(
    tx: ReadTx, artifacts: ArtifactStore, refs: Sequence[ResultOutputRef]
) -> dict[tuple[str, str], tuple[OutputMapping, ...]]:
    ids = tuple(dict.fromkeys(ref.result.id for ref in refs))
    values = await _mappings_in(tx, artifacts, ids)
    return {
        (ref.result.id, ref.output): tuple(
            item for item in values[ref.result.id] if item.output_name == ref.output
        )
        for ref in refs
    }


async def outputs_for_artifacts_in(
    tx: ReadTx, artifacts: ArtifactStore, refs: Sequence[ArtifactRef]
) -> dict[str, ResultOutputRef]:
    values = await artifacts.get_many_in(tx, refs)
    ids = list(dict.fromkeys(item.ref.id for item in values))
    output: dict[str, ResultOutputRef] = {}
    for start in range(0, len(ids), 800):
        rows = (
            await tx.connection.execute(
                sa.select(
                    schema.outputs.c.artifact_id,
                    schema.outputs.c.name,
                    schema.results.c.id,
                    schema.results.c.identifier,
                )
                .join(schema.results, schema.results.c.id == schema.outputs.c.result_id)
                .where(schema.outputs.c.artifact_id.in_(ids[start : start + 800]))
            )
        ).mappings()
        for row in rows:
            output[str(row["artifact_id"])] = ResultOutputRef(
                result=_ref(row, artifacts.instance_id), output=str(row["name"])
            )
    return output


async def get_many_in(
    tx: ReadTx, artifacts: ArtifactStore, refs: Sequence[str | ResultRef]
) -> tuple[ProcessingResult, ...]:
    """Read complete results, reusing the same source and mapping assembly as narrow reads."""
    requested = await _result_rows(tx, artifacts, refs, full=True)
    records = {str(row["id"]): row for row in requested}
    ids = tuple(records)
    if not ids:
        return ()
    selected = await _rows(tx, schema.selected_sources, ids, ("ordinal",))
    inputs = await _rows(tx, schema.inputs, ids, ("group_ordinal",))
    parts = await _rows(tx, schema.parts, ids, ("group_ordinal", "part_ordinal"))
    origins = await _rows(
        tx, schema.origins, ids, ("group_ordinal", "part_ordinal", "origin_ordinal")
    )
    outputs = await _rows(tx, schema.outputs, ids, ("ordinal",))
    named = await _rows(tx, schema.named_selections, ids, ("ordinal",))
    dependencies = await _rows(tx, schema.dependencies, ids, ("ordinal",))
    source_targets = await _rows(
        tx, schema.material_source_targets, ids, ("source_ordinal", "ordinal")
    )
    mapping_origins = await _rows(
        tx, schema.mapping_origins, ids, ("output_name", "mapping_ordinal", "ordinal")
    )
    identities = await _identities_in(
        tx, artifacts, parts, origins, outputs, dependencies, source_targets, mapping_origins
    )
    source_values, output_sources = await _sources_in(
        tx, artifacts, ids, source_targets, identities
    )
    mappings = await _mappings_in(tx, artifacts, ids, mapping_origins, identities)
    linked_ids = tuple(
        dict.fromkeys(
            str(row[column])
            for collection, column in (
                (origins, "transform_result_id"),
                (dependencies, "dependency_result_id"),
            )
            for rows in collection.values()
            for row in rows
            if row[column] is not None
        )
    )
    try:
        linked = {ref.id: ref for ref in await refs_in(tx, artifacts, linked_ids)}
    except NousError as error:
        if error.code == "content.not_found":
            raise NousError("content.provenance_missing", "A linked result is missing") from error
        raise
    assembled: dict[str, ProcessingResult] = {}
    for result_id, row in records.items():
        grouped_origins: dict[tuple[int, int], list[OriginSlice]] = {}
        for item in origins.get(result_id, ()):
            key = (int(item["group_ordinal"]), int(item["part_ordinal"]))
            grouped_origins.setdefault(key, []).append(
                OriginSlice(
                    origin=_content(item, identities),
                    transform_ref=linked.get(str(item["transform_result_id"])),
                    precision=item["precision"],
                )
            )
        grouped_parts: dict[int, list[InputPart]] = {}
        for item in parts.get(result_id, ()):
            ordinal = int(item["group_ordinal"])
            grouped_parts.setdefault(ordinal, []).append(
                InputPart(
                    content=_content(item, identities),
                    representation=str(item["representation"]),
                    media_type=item["media_type"],
                    source_media_type=item["source_media_type"],
                    origins=tuple(grouped_origins.get((ordinal, int(item["part_ordinal"])), ())),
                )
            )
        input_groups = tuple(
            InputGroup(
                key=str(item["key"]),
                slot_key=str(item["slot_key"]),
                role=item["role"],
                name=str(item["name"]),
                context=SourceContext.model_validate(item["context"]) if item["context"] else None,
                parts=tuple(grouped_parts[int(item["group_ordinal"])]),
            )
            for item in inputs.get(result_id, ())
        )
        result_outputs = tuple(
            ResultOutput(
                name=str(item["name"]),
                artifact=identities[str(item["artifact_id"])],
                role=item["role"],
                representation_kind=item["representation_kind"],
            )
            for item in outputs.get(result_id, ())
        )
        selected_inputs = SelectedInputs(
            sources=tuple(
                SelectedSource(
                    key=source.key,
                    material=source.material,
                    selections=source.selections,
                    kind=_SOURCE_ROLE.validate_python(source.kind),
                    name=source.name,
                    representations=tuple(
                        InputPart.model_validate(part) for part in (item["representations"] or ())
                    ),
                )
                for item in selected.get(result_id, ())
                for source in (source_values[result_id][int(item["source_ordinal"])],)
            ),
            facts=tuple(SourceFacts.model_validate(item) for item in row["selected_facts"]),
        )
        selections = tuple(
            NamedSelection(
                key=str(item["key"]),
                sources=tuple(ContentSelection.model_validate(value) for value in item["sources"]),
            )
            for item in named.get(result_id, ())
        )
        dependency_refs = tuple(
            identities[str(item["artifact_id"])]
            if item["artifact_id"] is not None
            else linked[str(item["dependency_result_id"])]
            for item in dependencies.get(result_id, ())
        )
        assembled[result_id] = ProcessingResult(
            ref=_ref(row, artifacts.instance_id),
            operation=str(row["operation"]),
            selected_inputs=selected_inputs,
            inputs=input_groups,
            outputs=result_outputs,
            mappings=mappings[result_id],
            selections=selections,
            material_sources=source_values[result_id],
            output_sources={
                output.name: output_sources.get((result_id, output.name), ())
                for output in result_outputs
            },
            dependencies=dependency_refs,
            producer=_PRODUCER.validate_python(row["producer"]),
            request=_JSON.validate_python(row["request"]),
            parameters=_JSON.validate_python(row["parameters"]),
            coverage=_JSON.validate_python(row["coverage"]),
            formed_at=cast(datetime, date_value(row["formed_at"])),
            recorded_at=cast(datetime, date_value(row["recorded_at"])),
        )
    return tuple(assembled[str(row["id"])] for row in requested)
