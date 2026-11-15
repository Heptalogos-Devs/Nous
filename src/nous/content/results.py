"""Atomic adoption and reading of completed processing results."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from uuid import uuid4

import sqlalchemy as sa
from pydantic import JsonValue, TypeAdapter

from nous.common.errors import NousError
from nous.common.identifier import insert_identifier
from nous.common.material_refs import RecordRevisionRef, WorkRevisionRef
from nous.common.operations import record_completed
from nous.storage.content import PublishedContent
from nous.storage.database import Database, ReadTx, WriteTx

from . import result_reading, schema
from .artifacts import ArtifactStore
from .contracts import (
    ArtifactRef,
    ContentSelection,
    MaterialSource,
    OutputContent,
    OutputMapping,
    OutputRole,
    ProcessingResult,
    ProducerFacts,
    ResultFacts,
    ResultOutput,
    ResultOutputRef,
    ResultOutputView,
    ResultRef,
    ResultView,
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
                    schema.results.c.cache_key == cache_key,
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
            cache_key = schema.results.c.cache_key
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
        output_sources: Mapping[str, Sequence[MaterialSource]] | None = None,
        cache_key: str | None = None,
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
        inputs, mappings = facts.inputs, facts.mappings
        selected = facts.selected_inputs.sources
        dependencies = tuple(
            dict.fromkeys(
                (
                    *facts.dependencies,
                    *(
                        origin.transform_ref
                        for source in selected
                        for part in source.representations
                        for origin in part.origins
                        if origin.transform_ref is not None
                    ),
                )
            )
        )
        slots = [source.key for source in selected]
        if len(slots) != len(set(slots)):
            raise NousError("content.duplicate_source", "Selected source slots must be unique")
        if any(group.slot_key not in slots for group in inputs):
            raise NousError("content.source_mismatch", "Each input group must have a selected slot")
        selected_refs = [
            selection.artifact for source in selected for selection in source.input_selections
        ]
        selected_identities = set(selected_refs)
        if any(item.artifact not in selected_identities for item in facts.selected_inputs.facts):
            raise NousError(
                "content.source_mismatch", "Source facts must describe a selected artifact"
            )
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
        output_sources = output_sources or {}
        if not set(output_sources).issubset(outputs_by_name):
            raise NousError("content.not_found", "Material source output does not exist")

        def formal(source: MaterialSource) -> MaterialSource:
            return MaterialSource(
                key=source.key,
                material=source.material,
                selections=source.selections,
                kind=source.kind,
                name=source.name,
            )

        output_source_values = {
            name: tuple(
                formal(source) for source in output_sources.get(name, facts.material_sources)
            )
            for name in outputs_by_name
        }
        if any(
            len({source.key for source in values}) != len(values)
            for values in output_source_values.values()
        ):
            raise NousError("content.duplicate_source", "Output source keys must be unique")
        selected_sources = tuple(formal(source) for source in selected)
        material_sources = list(
            dict.fromkeys(
                (
                    *(formal(source) for source in facts.material_sources),
                    *selected_sources,
                    *(source for values in output_source_values.values() for source in values),
                )
            )
        )
        source_ordinals = {
            name: tuple(material_sources.index(source) for source in values)
            for name, values in output_source_values.items()
        }
        output_source_values = {
            name: tuple(material_sources[index] for index in ordinals)
            for name, ordinals in source_ordinals.items()
        }
        material_selections = tuple(
            selection for source in material_sources for selection in source.selections
        )
        selection_keys = [item.key for item in facts.selections]
        if len(selection_keys) != len(set(selection_keys)):
            raise NousError("content.duplicate_selection", "Selection keys must be unique")
        selection_refs = [source.artifact for item in facts.selections for source in item.sources]
        artifact_refs = list(
            dict.fromkeys(
                (
                    *selected_refs,
                    *(
                        origin.origin.artifact
                        for source in selected
                        for part in source.representations
                        for origin in part.origins
                    ),
                    *(item.artifact for item in material_selections),
                    *selection_refs,
                    *(part.content.artifact for group in inputs for part in group.parts),
                    *(
                        origin.origin.artifact
                        for group in inputs
                        for part in group.parts
                        for origin in part.origins
                    ),
                    *(origin.artifact for mapping in mappings for origin in mapping.origins),
                    *(item for item in dependencies if isinstance(item, ArtifactRef)),
                )
            )
        )
        result_refs = tuple(
            dict.fromkeys(
                (
                    *(
                        origin.transform_ref
                        for group in inputs
                        for part in group.parts
                        for origin in part.origins
                        if origin.transform_ref is not None
                    ),
                    *(item for item in dependencies if isinstance(item, ResultRef)),
                    *(
                        source.material.result
                        for source in material_sources
                        if isinstance(source.material, ResultOutputRef)
                    ),
                )
            )
        )
        if any(
            ref.instance_id != self.artifacts.instance_id for ref in (*artifact_refs, *result_refs)
        ):
            raise NousError("content.instance_mismatch", "Result input belongs to another instance")
        await self.artifacts.get_many_in(tx, artifact_refs)
        mapping_artifacts = tuple(
            dict.fromkeys(origin.artifact for mapping in mappings for origin in mapping.origins)
        )
        mapping_blobs = await self.artifacts.content.resolve_many_in(
            tx, [item.id for item in mapping_artifacts]
        )
        blobs = {ref.id: blob for ref, blob in zip(mapping_artifacts, mapping_blobs, strict=True)}
        result_ids = list(dict.fromkeys(ref.id for ref in result_refs))
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
        if any(found_results[ref.id] != ref.identifier for ref in result_refs):
            raise NousError(
                "content.reference_mismatch", "Result reference identity does not match"
            )
        allowed = tuple(
            [selection for source in selected for selection in source.selections]
            + [selection for source in selected for selection in source.input_selections]
            + [part.content for group in inputs for part in group.parts]
            + [origin.origin for group in inputs for part in group.parts for origin in part.origins]
            + list(material_selections)
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
            if any(not included(origin) for origin in mapping.origins):
                raise NousError(
                    "content.invalid_mapping", "Mapping origin is outside the task inputs"
                )
            for origin in mapping.origins:
                blob = blobs[origin.artifact.id]
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
        if cache_key is not None:
            cached = await tx.connection.scalar(
                sa.select(schema.results.c.identifier).where(
                    schema.results.c.operation == facts.operation,
                    schema.results.c.cache_key == cache_key,
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
                "cache_key": cache_key,
                **values,
                "formed_at": facts.formed_at.isoformat(),
                "recorded_at": recorded.isoformat(),
            },
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
        material_rows = [
            {
                "result_id": object_id,
                "ordinal": ordinal,
                "key": source.key,
                "kind": source.kind,
                "name": source.name,
                "material": source.material.model_dump(mode="json"),
                "source_result_id": source.material.result.id
                if isinstance(source.material, ResultOutputRef)
                else None,
                "source_record_revision_id": source.material.revision_id
                if isinstance(source.material, RecordRevisionRef)
                else None,
                "source_work_revision_id": source.material.revision_id
                if isinstance(source.material, WorkRevisionRef)
                else None,
            }
            for ordinal, source in enumerate(material_sources)
        ]
        if material_rows:
            await tx.connection.execute(schema.material_sources.insert(), material_rows)
            await tx.connection.execute(
                schema.material_source_targets.insert(),
                [
                    {
                        "result_id": object_id,
                        "source_ordinal": source_ordinal,
                        "ordinal": ordinal,
                        "artifact_id": selection.artifact.id,
                        "selector": selection.selection.model_dump(mode="json"),
                    }
                    for source_ordinal, source in enumerate(material_sources)
                    for ordinal, selection in enumerate(source.selections)
                ],
            )
            output_rows = [
                {
                    "result_id": object_id,
                    "output_name": name,
                    "source_ordinal": source_ordinal,
                    "ordinal": ordinal,
                }
                for name, ordinals in source_ordinals.items()
                for ordinal, source_ordinal in enumerate(ordinals)
            ]
            if output_rows:
                await tx.connection.execute(schema.material_source_outputs.insert(), output_rows)
        if selected_sources:
            await tx.connection.execute(
                schema.selected_sources.insert(),
                [
                    {
                        "result_id": object_id,
                        "ordinal": ordinal,
                        "source_ordinal": material_sources.index(source),
                        "representations": [
                            part.model_dump(mode="json")
                            for part in selected[ordinal].representations
                        ]
                        if selected[ordinal].representations
                        else None,
                    }
                    for ordinal, source in enumerate(selected_sources)
                ],
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
        result = ProcessingResult(
            ref=ResultRef(
                id=object_id, identifier=identifier, instance_id=self.artifacts.instance_id
            ),
            operation=facts.operation,
            producer=facts.producer,
            selected_inputs=facts.selected_inputs,
            inputs=facts.inputs,
            outputs=tuple(result_outputs),
            mappings=facts.mappings,
            selections=facts.selections,
            material_sources=tuple(material_sources),
            output_sources=output_source_values,
            dependencies=dependencies,
            request=facts.request,
            parameters=facts.parameters,
            coverage=facts.coverage,
            formed_at=facts.formed_at,
            recorded_at=recorded,
        )
        return result

    async def adopt(
        self,
        *,
        facts: ResultFacts,
        outputs: Mapping[str, OutputContent],
        output_sources: Mapping[str, Sequence[MaterialSource]] | None = None,
        cache_key: str | None = None,
    ) -> ProcessingResult:
        """Publish completed outputs and commit their facts in a short transaction."""
        prepared = await self.prepare_outputs(outputs)
        async with self.database.write() as tx:
            result = await self.adopt_in(
                tx,
                facts=facts,
                prepared_outputs=prepared,
                output_sources=output_sources,
                cache_key=cache_key,
            )
        record_completed(result.ref)
        return result

    async def delete_in(self, tx: WriteTx, ref: str | ResultRef) -> ResultRef:
        """Remove one completed result; database references protect downstream facts."""
        result = (await self.refs_in(tx, (ref,)))[0]
        for table in (
            schema.origins,
            schema.parts,
            schema.inputs,
            schema.selected_sources,
            schema.material_source_outputs,
            schema.material_source_targets,
            schema.material_sources,
            schema.mapping_origins,
            schema.output_mappings,
            schema.outputs,
            schema.named_selections,
            schema.dependencies,
            schema.results,
        ):
            column = table.c.id if table is schema.results else table.c.result_id
            await tx.connection.execute(table.delete().where(column == result.id))
        return result

    async def resolve_output(
        self, result: str | ResultRef, output: str | None
    ) -> tuple[ResultOutputRef, ArtifactRef]:
        """Resolve a result and named output in one read."""
        async with self.database.read() as tx:
            return (await result_reading.outputs_in(tx, self.artifacts, ((result, output),)))[0]

    async def resolve_outputs_in(
        self, tx: ReadTx, requests: Sequence[tuple[str | ResultRef, str | None]]
    ) -> tuple[tuple[ResultOutputRef, ArtifactRef], ...]:
        return await result_reading.outputs_in(tx, self.artifacts, requests)

    async def refs_in(self, tx: ReadTx, refs: Sequence[str | ResultRef]) -> tuple[ResultRef, ...]:
        return await result_reading.refs_in(tx, self.artifacts, refs)

    async def refs(self, refs: Sequence[str | ResultRef]) -> tuple[ResultRef, ...]:
        async with self.database.read() as tx:
            return await self.refs_in(tx, refs)

    async def source_facts(
        self, ref: str | ResultRef, output: str, *, additional_kinds: tuple[str, ...] = ()
    ) -> tuple[
        ResultRef,
        str,
        dict[str, JsonValue],
        tuple[MaterialSource, ...],
        tuple[MaterialSource, ...] | None,
    ]:
        """Read a result header and one output's formal source relations."""
        async with self.database.read() as tx:
            return await result_reading.source_facts_in(
                tx, self.artifacts, ref, output, additional_kinds=additional_kinds
            )

    async def output_sources_in(
        self, tx: ReadTx, refs: Sequence[ResultOutputRef]
    ) -> dict[tuple[str, str], tuple[MaterialSource, ...]]:
        return await result_reading.output_sources_in(tx, self.artifacts, refs)

    async def output_mappings_in(
        self, tx: ReadTx, refs: Sequence[ResultOutputRef]
    ) -> dict[tuple[str, str], tuple[OutputMapping, ...]]:
        return await result_reading.output_mappings_in(tx, self.artifacts, refs)

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
    ) -> ResultView:
        """Read named output bodies and their exact sources for any product entrance."""
        async with self.artifacts.content.execution.operation("content.result_read"):
            result = await self.get(ref)
            outputs = tuple(
                output for output in result.outputs if details or output.role != "diagnostic"
            )
            output_views = await self.artifacts.read_view_many(
                [ContentSelection(artifact=output.artifact) for output in outputs],
                representations="none",
            )
            attached = (
                tuple(
                    source
                    for output in result.outputs
                    for source in result.sources_for(output.name)
                )
                if sources
                else ()
            )
            selected = tuple(
                MaterialSource(
                    key=source.key,
                    material=source.material,
                    selections=source.selections,
                    kind=source.kind,
                    name=source.name,
                )
                for source in result.selected_inputs.sources
            )
            show_selected = (
                sources and details and any(source not in attached for source in selected)
            )
            selections = (
                tuple(
                    dict.fromkeys(
                        (
                            *(item for source in attached for item in source.selections),
                            *(
                                (
                                    item
                                    for source in result.material_sources
                                    for item in source.selections
                                )
                                if details
                                else ()
                            ),
                            *(
                                (part.content for group in result.inputs for part in group.parts)
                                if details
                                else ()
                            ),
                            *(
                                (item for named in result.selections for item in named.sources)
                                if details
                                else ()
                            ),
                        )
                    )
                )
                if sources
                else ()
            )
            return ResultView(
                ref=result.ref,
                operation=result.operation,
                producer=result.producer,
                selected_inputs=result.selected_inputs if show_selected else None,
                inputs=result.inputs if details else None,
                outputs=tuple(
                    ResultOutputView(
                        name=output.name,
                        artifact=output.artifact,
                        role=output.role,
                        representation_kind=output.representation_kind,
                        view=view,
                        sources=result.sources_for(output.name) if sources else None,
                    )
                    for output, view in zip(outputs, output_views, strict=True)
                ),
                mappings=result.mappings,
                selections=result.selections,
                sources=tuple(
                    source
                    for source in result.material_sources
                    if source not in attached and (not show_selected or source not in selected)
                )
                if sources and details
                else (),
                dependencies=result.dependencies,
                request=result.request if details else {},
                parameters=result.parameters,
                coverage=result.coverage,
                formed_at=result.formed_at,
                recorded_at=result.recorded_at,
                content_views=await self.artifacts.read_view_many(selections) if sources else (),
            )

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
