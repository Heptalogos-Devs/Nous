"""Atomic adoption and reading of completed processing results."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime
from typing import cast
from uuid import uuid4

import sqlalchemy as sa
from pydantic import JsonValue, TypeAdapter

from nous.common.address import insert_address
from nous.common.errors import NousError
from nous.common.operations import record_completed
from nous.storage.content import PublishedContent
from nous.storage.database import Database, WriteTx

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
    ProcessingResult,
    ResultOutput,
    ResultRef,
    SourceSelection,
)
from .provenance import ProvenanceReader, ProvenanceSummary

_JSON = TypeAdapter(dict[str, JsonValue])


@dataclass(frozen=True)
class PreparedOutput:
    name: str
    content: PublishedContent
    media_type: str
    display_name: str


@dataclass(frozen=True)
class PreparedOutputs:
    outputs: tuple[PreparedOutput, ...]
    formed_at: datetime


class ResultStore:
    """Adopt completed stages with their ordered inputs, outputs and provenance."""

    def __init__(self, database: Database, artifacts: ArtifactStore) -> None:
        self.database, self.artifacts = database, artifacts
        self._provenance = ProvenanceReader(database, artifacts)

    async def find_cached(self, operation: str, cache_key: str) -> ProcessingResult | None:
        async with self.database.read() as tx:
            address = await tx.connection.scalar(
                sa.select(schema.results.c.address)
                .where(
                    schema.results.c.operation == operation,
                    schema.results.c.parameters["cache_key"].as_string() == cache_key,
                )
                .limit(1)
            )
        return await self.get(str(address)) if address is not None else None

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
                )
            )
        return PreparedOutputs(tuple(prepared), self.artifacts.clock.utc_now())

    async def adopt_in(
        self,
        tx: WriteTx,
        *,
        operation: str,
        producer: Mapping[str, object],
        inputs: Sequence[InputGroup],
        prepared_outputs: PreparedOutputs,
        request: Mapping[str, object] | None = None,
        parameters: Mapping[str, object] | None = None,
        coverage: Mapping[str, object] | None = None,
    ) -> ProcessingResult:
        """Register the result, output artifacts and body bindings in one caller-owned transaction.

        Returned references become committed only when that transaction exits successfully.
        """
        if not prepared_outputs.outputs:
            raise NousError("content.empty_result", "A result must have at least one output")
        keys = [group.key for group in inputs]
        if len(keys) != len(set(keys)):
            raise NousError(
                "content.duplicate_source", "Input group keys must be unique in a result"
            )
        identifier, recorded = str(uuid4()), self.artifacts.clock.utc_now()
        values = {
            "producer": _JSON.validate_python(dict(producer)),
            "request": _JSON.validate_python(dict(request or {})),
            "parameters": _JSON.validate_python(dict(parameters or {})),
            "coverage": _JSON.validate_python(dict(coverage or {})),
        }
        address = await insert_address(
            tx,
            schema.results,
            "res",
            {
                "id": identifier,
                "operation": operation,
                **values,
                "formed_at": prepared_outputs.formed_at.isoformat(),
                "recorded_at": recorded.isoformat(),
            },
        )
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
        for table, identifiers in (
            (schema.artifacts, list({ref.id for ref in artifact_refs})),
            (schema.results, list({ref.id for ref in transform_refs})),
        ):
            found_ids: set[str] = set()
            for start in range(0, len(identifiers), 900):
                found_ids.update(
                    str(value)
                    for value in (
                        await tx.connection.execute(
                            sa.select(table.c.id).where(
                                table.c.id.in_(identifiers[start : start + 900])
                            )
                        )
                    ).scalars()
                )
            if found_ids != set(identifiers):
                raise NousError("content.not_found", "An input relation refers to a missing record")
        group_rows: list[dict[str, object]] = []
        part_rows: list[dict[str, object]] = []
        origin_rows: list[dict[str, object]] = []
        for ordinal, group in enumerate(inputs):
            group_rows.append(
                {
                    "result_id": identifier,
                    "group_ordinal": ordinal,
                    "key": group.key,
                    "role": group.role,
                    "name": group.name,
                }
            )
            for part_ordinal, part in enumerate(group.parts):
                part_rows.append(
                    {
                        "result_id": identifier,
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
                            "result_id": identifier,
                            "group_ordinal": ordinal,
                            "part_ordinal": part_ordinal,
                            "origin_ordinal": origin_ordinal,
                            "artifact_id": origin.origin.artifact.id,
                            "selector": origin.origin.selection.model_dump(mode="json"),
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
                formed_at=prepared_outputs.formed_at,
            )
            await tx.connection.execute(
                schema.outputs.insert().values(
                    result_id=identifier,
                    ordinal=ordinal,
                    name=output.name,
                    artifact_id=artifact.ref.id,
                )
            )
            result_outputs.append(ResultOutput(name=output.name, artifact=artifact.ref))
        return ProcessingResult(
            ref=ResultRef(id=identifier, address=address, instance_id=self.artifacts.instance_id),
            operation=operation,
            inputs=tuple(inputs),
            outputs=tuple(result_outputs),
            **values,
            formed_at=prepared_outputs.formed_at,
            recorded_at=recorded,
        )

    async def adopt(
        self,
        *,
        operation: str,
        producer: Mapping[str, object],
        inputs: Sequence[InputGroup],
        outputs: Mapping[str, OutputContent],
        request: Mapping[str, object] | None = None,
        parameters: Mapping[str, object] | None = None,
        coverage: Mapping[str, object] | None = None,
    ) -> ProcessingResult:
        prepared = await self.prepare_outputs(outputs)
        async with self.database.write() as tx:
            result = await self.adopt_in(
                tx,
                operation=operation,
                producer=producer,
                inputs=inputs,
                prepared_outputs=prepared,
                request=request,
                parameters=parameters,
                coverage=coverage,
            )
        record_completed(result.ref.address)
        return result

    async def get(self, ref: str | ResultRef) -> ProcessingResult:
        if isinstance(ref, ResultRef):
            if ref.instance_id != self.artifacts.instance_id:
                raise NousError("content.instance_mismatch", "Result belongs to another instance")
            target = ref.id
        else:
            target = ref
        async with self.database.read() as tx:
            row = (
                (
                    await tx.connection.execute(
                        sa.select(schema.results).where(
                            sa.or_(
                                schema.results.c.id == target, schema.results.c.address == target
                            )
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
            identifiers = {
                str(source["artifact_id"]) for source in [*part_rows, *origin_rows, *output_rows]
            }
            identities: dict[str, ArtifactRef] = {}
            identity_list = list(identifiers)
            for start in range(0, len(identity_list), 900):
                selected = identity_list[start : start + 900]
                artifact_rows = (
                    await tx.connection.execute(
                        sa.select(schema.artifacts.c.id, schema.artifacts.c.address).where(
                            schema.artifacts.c.id.in_(selected)
                        )
                    )
                ).mappings()
                identities.update(
                    {
                        str(item["id"]): ArtifactRef(
                            id=str(item["id"]),
                            address=str(item["address"]),
                            instance_id=self.artifacts.instance_id,
                        )
                        for item in artifact_rows
                    }
                )
            if len(identities) != len(identifiers):
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
                        sa.select(schema.results.c.id, schema.results.c.address).where(
                            schema.results.c.id.in_(transform_ids)
                        )
                    )
                ).mappings()
                transforms = {
                    str(item["id"]): ResultRef(
                        id=str(item["id"]),
                        address=str(item["address"]),
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
                    role=source["role"],
                    name=str(source["name"]),
                    parts=tuple(grouped[int(source["group_ordinal"])]),
                )
                for source in input_rows
            ]
            outputs = [
                ResultOutput(
                    name=str(output["name"]), artifact=identities[str(output["artifact_id"])]
                )
                for output in output_rows
            ]
            return ProcessingResult(
                ref=ResultRef(
                    id=str(row["id"]),
                    address=str(row["address"]),
                    instance_id=self.artifacts.instance_id,
                ),
                operation=str(row["operation"]),
                inputs=tuple(inputs),
                outputs=tuple(outputs),
                producer=_JSON.validate_python(row["producer"]),
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
        result = await self.get(ref)
        value = result.model_dump(mode="json")
        if not sources:
            value.pop("inputs", None)
        if not details:
            value.pop("request", None)
        rendered: list[dict[str, object]] = []
        for output in result.outputs:
            artifact = await self.artifacts.get(output.artifact)
            blob = await self.artifacts.blob(output.artifact)
            item: dict[str, object] = {
                "name": output.name,
                "artifact": artifact.ref.address,
                "media_type": artifact.media_type,
                "byte_length": blob.byte_length,
            }
            if output.name != "response" and (
                artifact.media_type.startswith("text/") or artifact.media_type == "application/json"
            ):
                item["text"] = await self.artifacts.read_text(output.artifact)
            rendered.append(item)
        value["outputs"] = rendered
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
                            schema.results.c.address,
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
