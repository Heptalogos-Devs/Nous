"""Atomic adoption and reading of completed processing results."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime
from typing import cast
from uuid import uuid4

import sqlalchemy as sa
from pydantic import JsonValue, TypeAdapter

from nous.common.errors import NousError
from nous.common.identifier import insert_identifier
from nous.common.operations import record_completed
from nous.storage.content import PublishedContent
from nous.storage.database import Database, ReadTx, WriteTx

from . import schema
from ._records import date_value
from .artifacts import ArtifactStore
from .contracts import (
    ArtifactRef,
    ContentSelection,
    InputGroup,
    InputPart,
    OriginSlice,
    OutputContent,
    OutputMapping,
    OutputRole,
    ProcessingResult,
    ProducerFacts,
    ResultFacts,
    ResultOutput,
    ResultOutputRef,
    ResultRef,
    SelectedInputs,
    SelectedSource,
    SourceContext,
    SourceFacts,
    SourceSelection,
)
from .provenance import ProvenanceReader, ProvenanceSummary

_JSON = TypeAdapter(dict[str, JsonValue])
_PRODUCER = TypeAdapter(ProducerFacts)


@dataclass(frozen=True)
class PreparedOutput:
    name: str
    content: PublishedContent
    media_type: str
    display_name: str
    role: OutputRole
    representation_kind: str | None


@dataclass(frozen=True)
class PreparedOutputs:
    outputs: tuple[PreparedOutput, ...]


class ResultStore:
    """Adopt completed stages with their ordered inputs, outputs and provenance."""

    def __init__(self, database: Database, artifacts: ArtifactStore) -> None:
        self.database, self.artifacts = database, artifacts
        self._provenance = ProvenanceReader(database, artifacts)

    async def find_cached(self, operation: str, cache_key: str) -> ProcessingResult | None:
        async with self.database.read() as tx:
            identifier = await tx.connection.scalar(
                sa.select(schema.results.c.identifier)
                .where(
                    schema.results.c.operation == operation,
                    schema.results.c.parameters["cache_key"].as_string() == cache_key,
                )
                .limit(1)
            )
        return await self.get(str(identifier)) if identifier is not None else None

    async def cached_outputs_in(
        self, tx: ReadTx, operation: str, keys: Sequence[str], *, output: str
    ) -> dict[str, tuple[ResultRef, ArtifactRef]]:
        """Look up adopted output identities for multiple deterministic inputs in one read."""
        found: dict[str, tuple[ResultRef, ArtifactRef]] = {}
        unique = list(dict.fromkeys(keys))
        for start in range(0, len(unique), 400):
            cache_key = schema.results.c.parameters["cache_key"].as_string()
            rows = (
                await tx.connection.execute(
                    sa.select(
                        cache_key.label("cache_key"),
                        schema.results.c.id.label("result_id"),
                        schema.results.c.identifier.label("result_identifier"),
                        schema.artifacts.c.id.label("artifact_id"),
                        schema.artifacts.c.identifier.label("artifact_identifier"),
                    )
                    .select_from(schema.results.join(schema.outputs).join(schema.artifacts))
                    .where(
                        schema.results.c.operation == operation,
                        schema.outputs.c.name == output,
                        cache_key.in_(unique[start : start + 400]),
                    )
                    .order_by(schema.results.c.recorded_at, schema.results.c.id)
                )
            ).mappings()
            for row in rows:
                key = str(row["cache_key"])
                found.setdefault(
                    key,
                    (
                        ResultRef(
                            instance_id=self.artifacts.instance_id,
                            id=str(row["result_id"]),
                            identifier=str(row["result_identifier"]),
                        ),
                        ArtifactRef(
                            instance_id=self.artifacts.instance_id,
                            id=str(row["artifact_id"]),
                            identifier=str(row["artifact_identifier"]),
                        ),
                    ),
                )
        return found

    async def prepare_outputs(self, outputs: Mapping[str, OutputContent]) -> PreparedOutputs:
        """Publish output bytes before SQL; they remain reusable after an adoption rollback."""
        prepared: list[PreparedOutput] = []
        for name, output in outputs.items():
            prepared.append(
                PreparedOutput(
                    name,
                    await self.artifacts.content.publish_bytes(output.data),
                    output.media_type,
                    output.display_name,
                    output.role,
                    output.representation_kind,
                )
            )
        return PreparedOutputs(tuple(prepared))

    async def adopt_in(
        self,
        tx: WriteTx,
        *,
        facts: ResultFacts,
        prepared_outputs: PreparedOutputs,
    ) -> ProcessingResult:
        """Register the result, output artifacts and body bindings in one caller-owned transaction.

        Returns:
            The result and output references, committed only when the supplied
            transaction exits successfully.

        Raises:
            NousError: ``content.empty_result`` for no outputs,
                ``content.duplicate_source`` for repeated input group keys, or
                ``content.not_found`` for a missing input relation.
        """
        if not prepared_outputs.outputs:
            raise NousError("content.empty_result", "A result must have at least one output")
        inputs, mappings, dependencies = facts.inputs, facts.mappings, facts.dependencies
        selected = facts.selected_inputs.sources
        slots = [source.slot_key for source in selected]
        if len(slots) != len(set(slots)):
            raise NousError("content.duplicate_source", "Selected source slots must be unique")
        if any(group.slot_key not in slots for group in inputs):
            raise NousError("content.source_mismatch", "Each input group must have a selected slot")
        selected_refs = [
            selection.artifact for source in selected for selection in source.selections
        ]
        selected_identities = set(selected_refs)
        if any(item.artifact not in selected_identities for item in facts.selected_inputs.facts):
            raise NousError(
                "content.source_mismatch", "Source facts must describe a selected artifact"
            )
        if any(ref.instance_id != self.artifacts.instance_id for ref in selected_refs):
            raise NousError(
                "content.instance_mismatch", "Selected source belongs to another instance"
            )
        await self.artifacts.get_many_in(tx, selected_refs)
        keys = [group.key for group in inputs]
        if len(keys) != len(set(keys)):
            raise NousError(
                "content.duplicate_source", "Input group keys must be unique in a result"
            )
        outputs_by_name = {item.name: item for item in prepared_outputs.outputs}
        if len(outputs_by_name) != len(prepared_outputs.outputs) or any(
            not name for name in outputs_by_name
        ):
            raise NousError("content.duplicate_output", "Output names must be unique and nonempty")
        ranges: dict[str, list[tuple[int, int]]] = {}
        for mapping in mappings:
            output = outputs_by_name.get(mapping.output_name)
            start, end = mapping.selection.start_byte, mapping.selection.end_byte
            if (
                output is None
                or start is None
                or end is None
                or not 0 <= start < end <= output.content.blob.byte_length
            ):
                raise NousError(
                    "content.invalid_mapping", "Output mapping exceeds its named output"
                )
            if mapping.relation != "derived":
                ranges.setdefault(mapping.output_name, []).append((start, end))
            if any(
                origin.artifact.instance_id != self.artifacts.instance_id
                for origin in mapping.origins
            ):
                raise NousError(
                    "content.instance_mismatch", "Mapping origin belongs to another instance"
                )
            origin_artifacts = await self.artifacts.get_many_in(
                tx, [origin.artifact for origin in mapping.origins]
            )
            origin_blobs = await self.artifacts.content.resolve_many_in(
                tx, [item.ref.id for item in origin_artifacts]
            )
            for origin, blob in zip(mapping.origins, origin_blobs, strict=True):
                source = origin.selection
                if (
                    source.start_byte is not None
                    and source.start_byte > blob.byte_length
                    or source.end_byte is not None
                    and source.end_byte > blob.byte_length
                ):
                    raise NousError("content.invalid_mapping", "Mapping origin exceeds its source")
            if mapping.relation == "identity":
                source = mapping.origins[0].selection
                if (
                    len(mapping.origins) != 1
                    or source.start_byte is None
                    or source.end_byte is None
                    or source.end_byte - source.start_byte != end - start
                ):
                    raise NousError(
                        "content.invalid_mapping",
                        "Identity mapping requires equally sized exact byte ranges",
                    )
        if any(
            right[0] < left[1]
            for values in ranges.values()
            for left, right in zip(sorted(values), sorted(values)[1:], strict=False)
        ):
            raise NousError("content.invalid_mapping", "Output mapping ranges overlap")
        for dependency in dependencies:
            if dependency.instance_id != self.artifacts.instance_id:
                raise NousError(
                    "content.instance_mismatch", "Dependency belongs to another instance"
                )
            if isinstance(dependency, ArtifactRef):
                await self.artifacts.get_in(tx, dependency)
            else:
                await self.get_in(tx, dependency)
        artifact_refs = [part.content.artifact for group in inputs for part in group.parts]
        artifact_refs.extend(
            origin.origin.artifact
            for group in inputs
            for part in group.parts
            for origin in part.origins
        )
        transform_refs = [
            origin.transform_ref
            for group in inputs
            for part in group.parts
            for origin in part.origins
            if origin.transform_ref is not None
        ]
        if any(
            ref.instance_id != self.artifacts.instance_id
            for ref in [*artifact_refs, *transform_refs]
        ):
            raise NousError(
                "content.instance_mismatch", "Input relation belongs to another instance"
            )
        await self.artifacts.get_many_in(tx, artifact_refs)
        result_ids = list({ref.id for ref in transform_refs})
        found_results: dict[str, str] = {}
        for start in range(0, len(result_ids), 900):
            result_rows = await tx.connection.execute(
                sa.select(schema.results.c.id, schema.results.c.identifier).where(
                    schema.results.c.id.in_(result_ids[start : start + 900])
                )
            )
            found_results.update((str(row.id), str(row.identifier)) for row in result_rows)
        if len(found_results) != len(result_ids):
            raise NousError("content.not_found", "An input relation refers to a missing result")
        if any(found_results[ref.id] != ref.identifier for ref in transform_refs):
            raise NousError(
                "content.reference_mismatch", "Result reference identity does not match"
            )
        cache_key = facts.parameters.get("cache_key")
        if isinstance(cache_key, str):
            cached = await tx.connection.scalar(
                sa.select(schema.results.c.identifier).where(
                    schema.results.c.operation == facts.operation,
                    schema.results.c.parameters["cache_key"].as_string() == cache_key,
                )
            )
            if cached is not None:
                return await self.get_in(tx, str(cached))
        object_id, recorded = str(uuid4()), self.artifacts.clock.utc_now()
        values = {
            "producer": facts.producer.model_dump(mode="json"),
            "request": facts.request,
            "parameters": facts.parameters,
            "coverage": facts.coverage,
            "selected_facts": [
                item.model_dump(mode="json") for item in facts.selected_inputs.facts
            ],
        }
        identifier = await insert_identifier(
            tx,
            schema.results,
            "res",
            {
                "id": object_id,
                "operation": facts.operation,
                **values,
                "formed_at": facts.formed_at.isoformat(),
                "recorded_at": recorded.isoformat(),
            },
        )
        for ordinal, source in enumerate(selected):
            await tx.connection.execute(
                schema.selected_sources.insert().values(
                    result_id=object_id,
                    slot_key=source.slot_key,
                    ordinal=ordinal,
                    role=source.role,
                    name=source.display_name,
                )
            )
            await tx.connection.execute(
                schema.selected_targets.insert(),
                [
                    {
                        "result_id": object_id,
                        "slot_key": source.slot_key,
                        "ordinal": index,
                        "artifact_id": selection.artifact.id,
                        "selector": selection.selection.model_dump(mode="json"),
                    }
                    for index, selection in enumerate(source.selections)
                ],
            )
        group_rows: list[dict[str, object]] = []
        part_rows: list[dict[str, object]] = []
        origin_rows: list[dict[str, object]] = []
        for ordinal, group in enumerate(inputs):
            group_rows.append(
                {
                    "result_id": object_id,
                    "group_ordinal": ordinal,
                    "key": group.key,
                    "slot_key": group.slot_key,
                    "role": group.role,
                    "name": group.name,
                    "context": group.context.model_dump(mode="json") if group.context else None,
                }
            )
            for part_ordinal, part in enumerate(group.parts):
                part_rows.append(
                    {
                        "result_id": object_id,
                        "group_ordinal": ordinal,
                        "part_ordinal": part_ordinal,
                        "artifact_id": part.content.artifact.id,
                        "selector": part.content.selection.model_dump(mode="json"),
                        "representation": part.representation,
                        "media_type": part.media_type,
                        "source_media_type": part.source_media_type,
                    }
                )
                for origin_ordinal, origin in enumerate(part.origins):
                    origin_rows.append(
                        {
                            "result_id": object_id,
                            "group_ordinal": ordinal,
                            "part_ordinal": part_ordinal,
                            "origin_ordinal": origin_ordinal,
                            "artifact_id": origin.origin.artifact.id,
                            "selector": origin.origin.selection.model_dump(mode="json"),
                            "precision": origin.precision,
                            "transform_result_id": origin.transform_ref.id
                            if origin.transform_ref
                            else None,
                        }
                    )
        for table, rows in (
            (schema.inputs, group_rows),
            (schema.parts, part_rows),
            (schema.origins, origin_rows),
        ):
            if rows:
                await tx.connection.execute(table.insert(), rows)
        result_outputs: list[ResultOutput] = []
        for ordinal, output in enumerate(prepared_outputs.outputs):
            artifact = await self.artifacts.register_in(
                tx,
                output.content,
                media_type=output.media_type,
                display_name=output.display_name,
                source=None,
                received_at=None,
                formed_at=facts.formed_at,
            )
            await tx.connection.execute(
                schema.outputs.insert().values(
                    result_id=object_id,
                    ordinal=ordinal,
                    name=output.name,
                    artifact_id=artifact.ref.id,
                    role=output.role,
                    representation_kind=output.representation_kind,
                )
            )
            result_outputs.append(
                ResultOutput(
                    name=output.name,
                    artifact=artifact.ref,
                    role=output.role,
                    representation_kind=output.representation_kind,
                )
            )
        if mappings:
            ordinals: dict[str, int] = {}
            rows: list[dict[str, object]] = []
            for mapping in mappings:
                ordinal = ordinals.get(mapping.output_name, 0)
                ordinals[mapping.output_name] = ordinal + 1
                rows.append(
                    {
                        "result_id": object_id,
                        "output_name": mapping.output_name,
                        "ordinal": ordinal,
                        "selection": mapping.selection.model_dump(mode="json"),
                        "relation": mapping.relation,
                        "origins": [item.model_dump(mode="json") for item in mapping.origins],
                    }
                )
            await tx.connection.execute(schema.output_mappings.insert(), rows)
        if dependencies:
            await tx.connection.execute(
                schema.dependencies.insert(),
                [
                    {
                        "result_id": object_id,
                        "ordinal": ordinal,
                        "artifact_id": ref.id if isinstance(ref, ArtifactRef) else None,
                        "dependency_result_id": ref.id if isinstance(ref, ResultRef) else None,
                    }
                    for ordinal, ref in enumerate(dependencies)
                ],
            )
        return ProcessingResult(
            ref=ResultRef(
                id=object_id, identifier=identifier, instance_id=self.artifacts.instance_id
            ),
            **facts.model_dump(),
            outputs=tuple(result_outputs),
            recorded_at=recorded,
        )

    async def adopt(
        self,
        *,
        facts: ResultFacts,
        outputs: Mapping[str, OutputContent],
    ) -> ProcessingResult:
        """Publish completed outputs and commit their facts in a short transaction."""
        prepared = await self.prepare_outputs(outputs)
        async with self.database.write() as tx:
            result = await self.adopt_in(tx, facts=facts, prepared_outputs=prepared)
        record_completed(result.ref)
        return result

    async def resolve_output(self, ref: ResultOutputRef) -> ArtifactRef:
        """Resolve the named output without consulting its original producer."""
        async with self.database.read() as tx:
            return await self.resolve_output_in(tx, ref)

    async def resolve_output_in(self, tx: ReadTx, ref: ResultOutputRef) -> ArtifactRef:
        result = await self.get_in(tx, ref.result)
        for output in result.outputs:
            if output.name == ref.output:
                return output.artifact
        raise NousError(
            "content.not_found",
            "Result output does not exist",
            details={"result": ref.result.identifier, "output": ref.output},
        )

    async def get(self, ref: str | ResultRef) -> ProcessingResult:
        async with self.database.read() as tx:
            return await self.get_in(tx, ref)

    async def get_in(self, tx: ReadTx, ref: str | ResultRef) -> ProcessingResult:
        if isinstance(ref, ResultRef):
            if ref.instance_id != self.artifacts.instance_id:
                raise NousError("content.instance_mismatch", "Result belongs to another instance")
            target = ref.id
        else:
            target = ref
        row = (
            (
                await tx.connection.execute(
                    sa.select(schema.results).where(
                        sa.or_(schema.results.c.id == target, schema.results.c.identifier == target)
                    )
                )
            )
            .mappings()
            .one_or_none()
        )
        if row is None:
            raise NousError(
                "content.not_found", "Processing result does not exist", details={"ref": target}
            )
        if isinstance(ref, ResultRef) and ref.identifier != row["identifier"]:
            raise NousError(
                "content.reference_mismatch", "Result reference identity does not match"
            )
        selected_rows = (
            (
                await tx.connection.execute(
                    sa.select(schema.selected_sources)
                    .where(schema.selected_sources.c.result_id == row["id"])
                    .order_by(schema.selected_sources.c.ordinal)
                )
            )
            .mappings()
            .all()
        )
        target_rows = (
            (
                await tx.connection.execute(
                    sa.select(schema.selected_targets)
                    .where(schema.selected_targets.c.result_id == row["id"])
                    .order_by(schema.selected_targets.c.slot_key, schema.selected_targets.c.ordinal)
                )
            )
            .mappings()
            .all()
        )
        input_rows = (
            (
                await tx.connection.execute(
                    sa.select(schema.inputs)
                    .where(schema.inputs.c.result_id == row["id"])
                    .order_by(schema.inputs.c.group_ordinal)
                )
            )
            .mappings()
            .all()
        )
        part_rows = (
            (
                await tx.connection.execute(
                    sa.select(schema.parts)
                    .where(schema.parts.c.result_id == row["id"])
                    .order_by(schema.parts.c.group_ordinal, schema.parts.c.part_ordinal)
                )
            )
            .mappings()
            .all()
        )
        origin_rows = (
            (
                await tx.connection.execute(
                    sa.select(schema.origins)
                    .where(schema.origins.c.result_id == row["id"])
                    .order_by(
                        schema.origins.c.group_ordinal,
                        schema.origins.c.part_ordinal,
                        schema.origins.c.origin_ordinal,
                    )
                )
            )
            .mappings()
            .all()
        )
        output_rows = (
            (
                await tx.connection.execute(
                    sa.select(schema.outputs)
                    .where(schema.outputs.c.result_id == row["id"])
                    .order_by(schema.outputs.c.ordinal)
                )
            )
            .mappings()
            .all()
        )
        object_ids = {
            str(source["artifact_id"])
            for source in [*target_rows, *part_rows, *origin_rows, *output_rows]
        }
        identities: dict[str, ArtifactRef] = {}
        identity_list = list(object_ids)
        for start in range(0, len(identity_list), 900):
            selected = identity_list[start : start + 900]
            artifact_rows = (
                await tx.connection.execute(
                    sa.select(schema.artifacts.c.id, schema.artifacts.c.identifier).where(
                        schema.artifacts.c.id.in_(selected)
                    )
                )
            ).mappings()
            identities.update(
                {
                    str(item["id"]): ArtifactRef(
                        id=str(item["id"]),
                        identifier=str(item["identifier"]),
                        instance_id=self.artifacts.instance_id,
                    )
                    for item in artifact_rows
                }
            )
        if len(identities) != len(object_ids):
            raise NousError("content.provenance_missing", "A result artifact is missing")
        transform_ids = {
            str(source["transform_result_id"])
            for source in origin_rows
            if source["transform_result_id"] is not None
        }
        transforms: dict[str, ResultRef] = {}
        if transform_ids:
            transform_rows = (
                await tx.connection.execute(
                    sa.select(schema.results.c.id, schema.results.c.identifier).where(
                        schema.results.c.id.in_(transform_ids)
                    )
                )
            ).mappings()
            transforms = {
                str(item["id"]): ResultRef(
                    id=str(item["id"]),
                    identifier=str(item["identifier"]),
                    instance_id=self.artifacts.instance_id,
                )
                for item in transform_rows
            }
        origins: dict[tuple[int, int], list[OriginSlice]] = {}
        for source in origin_rows:
            key = (int(source["group_ordinal"]), int(source["part_ordinal"]))
            origins.setdefault(key, []).append(
                OriginSlice(
                    origin=ContentSelection(
                        artifact=identities[str(source["artifact_id"])],
                        selection=SourceSelection.model_validate(source["selector"]),
                    ),
                    transform_ref=transforms.get(str(source["transform_result_id"])),
                    precision=source["precision"],
                )
            )
        grouped: dict[int, list[InputPart]] = {}
        for source in part_rows:
            ordinal = int(source["group_ordinal"])
            grouped.setdefault(ordinal, []).append(
                InputPart(
                    content=ContentSelection(
                        artifact=identities[str(source["artifact_id"])],
                        selection=SourceSelection.model_validate(source["selector"]),
                    ),
                    representation=str(source["representation"]),
                    media_type=source["media_type"],
                    source_media_type=source["source_media_type"],
                    origins=tuple(origins.get((ordinal, int(source["part_ordinal"])), [])),
                )
            )
        inputs = [
            InputGroup(
                key=str(source["key"]),
                slot_key=str(source["slot_key"]),
                role=source["role"],
                name=str(source["name"]),
                context=SourceContext.model_validate(source["context"])
                if source["context"]
                else None,
                parts=tuple(grouped[int(source["group_ordinal"])]),
            )
            for source in input_rows
        ]
        outputs = [
            ResultOutput(
                name=str(output["name"]),
                artifact=identities[str(output["artifact_id"])],
                role=output["role"],
                representation_kind=output["representation_kind"],
            )
            for output in output_rows
        ]
        selected_by_slot: dict[str, list[ContentSelection]] = {}
        for target_row in target_rows:
            selected_by_slot.setdefault(str(target_row["slot_key"]), []).append(
                ContentSelection(
                    artifact=identities[str(target_row["artifact_id"])],
                    selection=SourceSelection.model_validate(target_row["selector"]),
                )
            )
        selected_inputs = SelectedInputs(
            sources=tuple(
                SelectedSource(
                    slot_key=str(source["slot_key"]),
                    role=source["role"],
                    display_name=source["name"],
                    selections=tuple(selected_by_slot[str(source["slot_key"])]),
                )
                for source in selected_rows
            ),
            facts=tuple(SourceFacts.model_validate(item) for item in row["selected_facts"]),
        )
        mapping_rows = (
            (
                await tx.connection.execute(
                    sa.select(schema.output_mappings)
                    .where(schema.output_mappings.c.result_id == row["id"])
                    .order_by(
                        schema.output_mappings.c.output_name, schema.output_mappings.c.ordinal
                    )
                )
            )
            .mappings()
            .all()
        )
        mappings = tuple(
            OutputMapping(
                output_name=str(item["output_name"]),
                selection=SourceSelection.model_validate(item["selection"]),
                relation=item["relation"],
                origins=tuple(
                    ContentSelection.model_validate(origin) for origin in item["origins"]
                ),
            )
            for item in mapping_rows
        )
        dependency_rows = (
            (
                await tx.connection.execute(
                    sa.select(schema.dependencies)
                    .where(schema.dependencies.c.result_id == row["id"])
                    .order_by(schema.dependencies.c.ordinal)
                )
            )
            .mappings()
            .all()
        )
        dependencies: list[ArtifactRef | ResultRef] = []
        for item in dependency_rows:
            if item["artifact_id"] is not None:
                dependencies.append((await self.artifacts.get_in(tx, str(item["artifact_id"]))).ref)
            else:
                dependency_row = (
                    await tx.connection.execute(
                        sa.select(schema.results.c.id, schema.results.c.identifier).where(
                            schema.results.c.id == item["dependency_result_id"]
                        )
                    )
                ).one()
                dependencies.append(
                    ResultRef(
                        id=str(dependency_row.id),
                        identifier=str(dependency_row.identifier),
                        instance_id=self.artifacts.instance_id,
                    )
                )
        return ProcessingResult(
            ref=ResultRef(
                id=str(row["id"]),
                identifier=str(row["identifier"]),
                instance_id=self.artifacts.instance_id,
            ),
            operation=str(row["operation"]),
            selected_inputs=selected_inputs,
            inputs=tuple(inputs),
            outputs=tuple(outputs),
            mappings=mappings,
            dependencies=tuple(dependencies),
            producer=_PRODUCER.validate_python(row["producer"]),
            request=_JSON.validate_python(row["request"]),
            parameters=_JSON.validate_python(row["parameters"]),
            coverage=_JSON.validate_python(row["coverage"]),
            formed_at=cast(datetime, date_value(row["formed_at"])),
            recorded_at=cast(datetime, date_value(row["recorded_at"])),
        )

    async def read_view(
        self, ref: str | ResultRef, *, sources: bool = True, details: bool = False
    ) -> dict[str, object]:
        """Read named output bodies and their exact sources for any product entrance."""
        async with self.artifacts.content.execution.operation("content.result_read"):
            result = await self.get(ref)
            value = result.model_dump(mode="json")
            if not details:
                value.pop("request", None)
            output_views = await self.artifacts.read_view_many(
                [
                    ContentSelection(artifact=output.artifact)
                    for output in result.outputs
                    if details or output.role != "diagnostic"
                ],
                representations="none",
            )
            value["outputs"] = [
                {
                    "name": output.name,
                    "artifact": output.artifact.model_dump(mode="json"),
                    "role": output.role,
                    "representation_kind": output.representation_kind,
                    "view": view.model_dump(mode="json"),
                }
                for output, view in zip(
                    (output for output in result.outputs if details or output.role != "diagnostic"),
                    output_views,
                    strict=True,
                )
            ]
            if sources:
                grouped = [source.selections for source in result.selected_inputs.sources]
                actual_groups = [
                    tuple(part.content for part in group.parts) for group in result.inputs
                ]
                all_views = await self.artifacts.read_view_many(
                    [item for group in (*grouped, *actual_groups) for item in group]
                )
                source_views: dict[str, object] = {}
                offset = 0
                for source, selected in zip(result.selected_inputs.sources, grouped, strict=True):
                    source_views[source.slot_key] = [
                        view.model_dump(mode="json")
                        for view in all_views[offset : offset + len(selected)]
                    ]
                    offset += len(selected)
                value["source_views"] = source_views
                input_views: dict[str, object] = {}
                for group, parts in zip(result.inputs, actual_groups, strict=True):
                    input_views[group.key] = [
                        view.model_dump(mode="json")
                        for view in all_views[offset : offset + len(parts)]
                    ]
                    offset += len(parts)
                value["input_views"] = input_views
            return value

    async def provenance(
        self, refs: Sequence[str | ArtifactRef], *, details: bool = False
    ) -> dict[str, ProvenanceSummary]:
        return await self._provenance.read(refs, details=details)

    async def recent(self, limit: int = 5) -> list[dict[str, JsonValue]]:
        """Summaries of adopted outputs, without their bodies or provenance."""
        async with self.database.read() as tx:
            rows = (
                (
                    await tx.connection.execute(
                        sa.select(
                            schema.results.c.identifier,
                            schema.results.c.operation,
                            schema.results.c.producer,
                            schema.results.c.coverage,
                            schema.results.c.recorded_at,
                        )
                        .order_by(schema.results.c.recorded_at.desc())
                        .limit(limit)
                    )
                )
                .mappings()
                .all()
            )
        return [_JSON.validate_python(dict(row)) for row in rows]
