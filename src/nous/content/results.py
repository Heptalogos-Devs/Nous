"""Atomic adoption and reading of completed processing results."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from uuid import uuid4

import sqlalchemy as sa
from pydantic import JsonValue, TypeAdapter

from nous.common.errors import NousError
from nous.common.identifier import insert_identifier
from nous.common.operations import record_completed
from nous.storage.content import PublishedContent
from nous.storage.database import Database, ReadTx, WriteTx

from . import result_reading, schema
from .artifacts import ArtifactStore
from .contracts import (
    ArtifactRef,
    ContentSelection,
    OutputContent,
    OutputRole,
    ProcessingResult,
    ProducerFacts,
    ResultFacts,
    ResultOutput,
    ResultOutputRef,
    ResultRef,
)
from .provenance import ProvenanceReader, ProvenanceSummary

_JSON = TypeAdapter(dict[str, JsonValue])
_PRODUCER = TypeAdapter(ProducerFacts)


async def upgrade_mapping_origins_in(tx: WriteTx) -> None:
    """Normalize existing output-mapping origins once in the same transaction."""

    def columns(connection: sa.Connection) -> set[str]:
        return {
            item["name"] for item in sa.inspect(connection).get_columns("content_output_mappings")
        }

    if "origins" not in await tx.connection.run_sync(columns):
        return
    invalid = await tx.connection.scalar(
        sa.text(
            "SELECT 1 FROM content_output_mappings "
            "WHERE COALESCE(json_type(origins), '') != 'array' OR json_array_length(origins) = 0 LIMIT 1"
        )
    )
    if invalid:
        raise NousError("content.invalid_mapping", "A saved output mapping has no valid origins")
    invalid = await tx.connection.scalar(
        sa.text(
            "SELECT 1 FROM content_output_mappings AS mapping, json_each(mapping.origins) AS origin "
            "LEFT JOIN content_artifacts AS artifact "
            "ON artifact.id = json_extract(origin.value, '$.artifact.id') "
            "WHERE artifact.id IS NULL "
            "OR artifact.identifier IS NOT json_extract(origin.value, '$.artifact.identifier') "
            "OR json_extract(origin.value, '$.artifact.instance_id') "
            "IS NOT (SELECT instance_id FROM instance_metadata) "
            "OR COALESCE(json_type(origin.value, '$.selection'), '') != 'object' LIMIT 1"
        )
    )
    if invalid:
        raise NousError("content.invalid_mapping", "A saved output mapping has an invalid source")
    await tx.connection.exec_driver_sql(
        "INSERT INTO content_output_mapping_origins "
        "(result_id, output_name, mapping_ordinal, ordinal, artifact_id, selector) "
        "SELECT mapping.result_id, mapping.output_name, mapping.ordinal, CAST(origin.key AS INTEGER), "
        "json_extract(origin.value, '$.artifact.id'), json_extract(origin.value, '$.selection') "
        "FROM content_output_mappings AS mapping, json_each(mapping.origins) AS origin"
    )
    await tx.connection.exec_driver_sql("DROP TABLE IF EXISTS processing_representation_origins")
    await tx.connection.exec_driver_sql("ALTER TABLE content_output_mappings DROP COLUMN origins")


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
        selection_keys = [item.key for item in facts.selections]
        if len(selection_keys) != len(set(selection_keys)):
            raise NousError("content.duplicate_selection", "Selection keys must be unique")
        selection_refs = [source.artifact for item in facts.selections for source in item.sources]
        if any(ref.instance_id != self.artifacts.instance_id for ref in selection_refs):
            raise NousError("content.instance_mismatch", "Selection belongs to another instance")
        await self.artifacts.get_many_in(tx, selection_refs)
        allowed = tuple(
            [selection for source in selected for selection in source.selections]
            + [part.content for group in inputs for part in group.parts]
            + [origin.origin for group in inputs for part in group.parts for origin in part.origins]
        )

        def included(selection: ContentSelection) -> bool:
            for candidate in allowed:
                if candidate.artifact != selection.artifact:
                    continue
                try:
                    candidate.selection.restrict(selection.selection)
                    return True
                except NousError:
                    continue
            return False

        if any(not included(source) for item in facts.selections for source in item.sources):
            raise NousError(
                "content.invalid_selection", "Named selection is outside the task inputs"
            )
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
            if any(not included(origin) for origin in mapping.origins):
                raise NousError(
                    "content.invalid_mapping", "Mapping origin is outside the task inputs"
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
            origins: list[dict[str, object]] = []
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
                    }
                )
                origins.extend(
                    {
                        "result_id": object_id,
                        "output_name": mapping.output_name,
                        "mapping_ordinal": ordinal,
                        "ordinal": index,
                        "artifact_id": origin.artifact.id,
                        "selector": origin.selection.model_dump(mode="json"),
                    }
                    for index, origin in enumerate(mapping.origins)
                )
            await tx.connection.execute(schema.output_mappings.insert(), rows)
            await tx.connection.execute(schema.mapping_origins.insert(), origins)
        if facts.selections:
            await tx.connection.execute(
                schema.named_selections.insert(),
                [
                    {
                        "result_id": object_id,
                        "ordinal": ordinal,
                        "key": item.key,
                        "sources": [source.model_dump(mode="json") for source in item.sources],
                    }
                    for ordinal, item in enumerate(facts.selections)
                ],
            )
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

    async def delete_in(self, tx: WriteTx, ref: str | ResultRef) -> ResultRef:
        """Remove one completed result; database references protect downstream facts."""
        result = await self.get_in(tx, ref)
        for table in (
            schema.origins,
            schema.parts,
            schema.inputs,
            schema.selected_targets,
            schema.selected_sources,
            schema.material_sources,
            schema.mapping_origins,
            schema.output_mappings,
            schema.outputs,
            schema.named_selections,
            schema.dependencies,
            schema.results,
        ):
            column = table.c.id if table is schema.results else table.c.result_id
            await tx.connection.execute(table.delete().where(column == result.ref.id))
        return result.ref

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

    async def outputs_for_artifacts_in(
        self, tx: ReadTx, refs: Sequence[ArtifactRef]
    ) -> dict[str, ResultOutputRef]:
        """Find the producing named output of each exact artifact in this read view."""
        return await result_reading.outputs_for_artifacts_in(tx, self.artifacts, refs)

    async def get(self, ref: str | ResultRef) -> ProcessingResult:
        async with self.database.read() as tx:
            return await self.get_in(tx, ref)

    async def get_many_in(
        self, tx: ReadTx, refs: Sequence[str | ResultRef]
    ) -> tuple[ProcessingResult, ...]:
        """Read complete result facts in request order using the caller's read view."""
        return await result_reading.get_many_in(tx, self.artifacts, refs)

    async def get_in(self, tx: ReadTx, ref: str | ResultRef) -> ProcessingResult:
        return (await self.get_many_in(tx, (ref,)))[0]

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
