"""Artifact acceptance and atomic adoption of completed processing stages."""

from __future__ import annotations

import mimetypes
from collections.abc import AsyncGenerator, Callable, Mapping, Sequence
from contextlib import asynccontextmanager
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import cast
from uuid import uuid4

import sqlalchemy as sa
from pydantic import JsonValue, TypeAdapter
from sqlalchemy.dialects.sqlite import insert

from nous.common.address import new_address
from nous.common.errors import NousError
from nous.storage.content import ContentStore, PublishedContent
from nous.storage.database import Database, ReadTx, WriteTx
from nous.temporal.values import Clock

from . import schema
from .configuration import ContentConfig
from .contracts import (
    Artifact,
    ArtifactRef,
    BlobRef,
    ContentSelection,
    InputGroup,
    InputSpec,
    OutputContent,
    ProcessingResult,
    ResultOutput,
    ResultRef,
)

_JSON = TypeAdapter(dict[str, JsonValue])


async def _insert_address(
    tx: WriteTx, table: sa.Table, prefix: str, values: Mapping[str, object]
) -> str:
    for _ in range(32):
        address = new_address(prefix)
        result = await tx.connection.execute(
            insert(table)
            .values(**values, address=address)
            .on_conflict_do_nothing(index_elements=[table.c.address])
        )
        if result.rowcount == 1:
            return address
    raise NousError("content.address_exhausted", "Cannot allocate a unique readable address")


def _date(value: object) -> datetime | None:
    return datetime.fromisoformat(str(value)) if value else None


class ArtifactStore:
    """Own artifact identities while CAS owns their authoritative body bindings."""

    def __init__(
        self,
        database: Database,
        content: ContentStore,
        instance_id: str,
        *,
        text_read_max_bytes: Callable[[], int] | None = None,
        clock: Clock | None = None,
    ) -> None:
        self.database, self.content, self.instance_id = database, content, instance_id
        self.clock = clock or Clock()
        self.text_read_max_bytes = text_read_max_bytes or (
            lambda: ContentConfig().text_read_max_bytes
        )

    async def ingest(
        self,
        source: Path | bytes | str,
        *,
        media_type: str | None = None,
        display_name: str | None = None,
        origin: str | None = None,
    ) -> Artifact:
        """Publish bytes, then record this acceptance in a short transaction."""
        if isinstance(source, Path):
            published = await self.content.publish_file(source)
            name = display_name or source.name
            media_type = media_type or mimetypes.guess_type(name)[0] or "application/octet-stream"
            origin = origin or str(source)
        else:
            data = str.encode(source, "utf-8") if isinstance(source, str) else source
            published = await self.content.publish_bytes(data)
            name = display_name or "input.txt"
            media_type = media_type or "text/plain"
        now = self.clock.utc_now()
        async with self.database.write() as tx:
            artifact = await self.register_in(
                tx,
                published,
                media_type=media_type,
                display_name=name,
                source=origin,
                received_at=now,
                formed_at=None,
            )
        return artifact

    async def register_in(
        self,
        tx: WriteTx,
        published: PublishedContent,
        *,
        media_type: str,
        display_name: str,
        source: str | None,
        received_at: datetime | None,
        formed_at: datetime | None,
    ) -> Artifact:
        """Register identity and body in the caller's transaction; the caller owns commit."""
        identifier, recorded = str(uuid4()), self.clock.utc_now()
        address = await _insert_address(
            tx,
            schema.artifacts,
            "art",
            {
                "id": identifier,
                "media_type": media_type,
                "display_name": display_name,
                "source": source,
                "received_at": received_at.isoformat() if received_at else None,
                "formed_at": formed_at.isoformat() if formed_at else None,
                "recorded_at": recorded.isoformat(),
            },
        )
        await self.content.attach_in(tx, identifier, "body", published)
        return Artifact(
            ref=ArtifactRef(id=identifier, address=address, instance_id=self.instance_id),
            media_type=media_type,
            display_name=display_name,
            source=source,
            received_at=received_at,
            formed_at=formed_at,
            recorded_at=recorded,
        )

    async def get(self, ref: str | ArtifactRef) -> Artifact:
        async with self.database.read() as tx:
            return await self.get_in(tx, ref)

    async def get_in(self, tx: ReadTx, ref: str | ArtifactRef) -> Artifact:
        if isinstance(ref, ArtifactRef):
            if ref.instance_id != self.instance_id:
                raise NousError("content.instance_mismatch", "Artifact belongs to another instance")
            target = ref.id
        else:
            target = ref
        row = (
            (
                await tx.connection.execute(
                    sa.select(schema.artifacts).where(
                        sa.or_(
                            schema.artifacts.c.id == target, schema.artifacts.c.address == target
                        )
                    )
                )
            )
            .mappings()
            .one_or_none()
        )
        if row is None:
            raise NousError("content.not_found", "Artifact does not exist", details={"ref": target})
        # Output links are the sole producer relation for derived artifacts.
        producer_row = (
            (
                await tx.connection.execute(
                    sa.select(schema.results.c.id, schema.results.c.address)
                    .join(schema.outputs)
                    .where(schema.outputs.c.artifact_id == row["id"])
                )
            )
            .mappings()
            .one_or_none()
        )
        producer = (
            ResultRef(instance_id=self.instance_id, **dict(producer_row)) if producer_row else None
        )
        return Artifact(
            ref=ArtifactRef(
                id=str(row["id"]), address=str(row["address"]), instance_id=self.instance_id
            ),
            media_type=str(row["media_type"]),
            display_name=str(row["display_name"]),
            source=str(row["source"]) if row["source"] is not None else None,
            received_at=_date(row["received_at"]),
            formed_at=_date(row["formed_at"]),
            recorded_at=cast(datetime, _date(row["recorded_at"])),
            producer_result_ref=producer,
        )

    async def blob(self, ref: str | ArtifactRef) -> BlobRef:
        async with self.database.read() as tx:
            artifact = await self.get_in(tx, ref)
            return await self.content.resolve_in(tx, artifact.ref.id)

    async def read_bytes(
        self, ref: str | ArtifactRef, *, max_bytes: int = 64 * 1024 * 1024
    ) -> bytes:
        return await self.content.read_bytes(await self.blob(ref), max_bytes=max_bytes)

    async def read_text(self, ref: str | ArtifactRef, *, max_bytes: int | None = None) -> str:
        return await self.content.read_text(
            await self.blob(ref), self.text_read_max_bytes() if max_bytes is None else max_bytes
        )

    async def accept(self, spec: InputSpec) -> Artifact:
        """Accept a declared user input through the same immutable-content path."""
        if spec.kind == "artifact":
            assert spec.artifact is not None
            return await self.get(spec.artifact)
        if spec.kind == "file":
            assert spec.path is not None
            return await self.ingest(Path(spec.path), display_name=spec.display_name)
        assert spec.text is not None
        return await self.ingest(spec.text, display_name=spec.display_name)

    async def read_selection(self, content: ContentSelection) -> str:
        """Read exact UTF-8 lines; reject coordinates requiring deterministic media preparation."""
        selection = content.selection
        if selection.pages or any(
            value is not None
            for value in (selection.start_ms, selection.end_ms, selection.frame, selection.crop)
        ):
            raise NousError(
                "content.selection_requires_preparation",
                "Media coordinates require media preparation",
            )
        if selection.start_byte is not None or selection.end_byte is not None:
            blob = await self.blob(content.artifact)
            start, end = (
                selection.start_byte or 0,
                selection.end_byte if selection.end_byte is not None else blob.byte_length,
            )
            if not 0 <= start <= end <= blob.byte_length:
                raise NousError("content.invalid_selection", "Byte selection exceeds the artifact")
            if end - start > self.text_read_max_bytes():
                raise NousError(
                    "content.too_large", "Text selection exceeds the configured read limit"
                )
            async with self.content.open(blob, start=start, end=end) as reader:
                data = await self.content.execution.blocking(reader.read)
            try:
                return data.decode("utf-8")
            except UnicodeDecodeError as exc:
                raise NousError(
                    "content.not_text", "Byte selection does not contain complete UTF-8 text"
                ) from exc
        text = await self.read_text(content.artifact)
        if selection.start_line is None and selection.end_line is None:
            return text
        lines = text.splitlines(keepends=True)
        first, last = selection.start_line or 1, selection.end_line or len(lines)
        if first > len(lines) or last > len(lines):
            raise NousError("content.invalid_selection", "Text selection exceeds the artifact")
        return "".join(lines[first - 1 : last])

    @asynccontextmanager
    async def borrow_local(self, ref: str | ArtifactRef) -> AsyncGenerator[Path]:
        async with self.content.borrow_local(await self.blob(ref)) as path:
            yield path

    async def export(self, ref: str | ArtifactRef, destination: Path) -> Path:
        return await self.content.export(await self.blob(ref), destination)


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
        address = await _insert_address(
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
        for ordinal, group in enumerate(inputs):
            for part in group.parts:
                await self.artifacts.get_in(tx, part.content.artifact)
                for origin in part.origins:
                    await self.artifacts.get_in(tx, origin.origin.artifact)
                    if origin.transform_ref is not None:
                        if origin.transform_ref.instance_id != self.artifacts.instance_id:
                            raise NousError(
                                "content.instance_mismatch", "Transform belongs to another instance"
                            )
                        found = await tx.connection.scalar(
                            sa.select(schema.results.c.id).where(
                                schema.results.c.id == origin.transform_ref.id
                            )
                        )
                        if found is None:
                            raise NousError("content.not_found", "Transform result does not exist")
            await tx.connection.execute(
                schema.inputs.insert().values(
                    result_id=identifier, ordinal=ordinal, group=group.model_dump(mode="json")
                )
            )
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
                        .order_by(schema.inputs.c.ordinal)
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
            inputs = [InputGroup.model_validate(source["group"]) for source in input_rows]
            outputs: list[ResultOutput] = []
            for output in output_rows:
                artifact = await self.artifacts.get_in(tx, str(output["artifact_id"]))
                outputs.append(ResultOutput(name=str(output["name"]), artifact=artifact.ref))
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
                formed_at=cast(datetime, _date(row["formed_at"])),
                recorded_at=cast(datetime, _date(row["recorded_at"])),
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
        self, refs: Sequence[str | ArtifactRef]
    ) -> dict[str, tuple[ArtifactRef, ...]]:
        """Resolve terminal evidence roots from a single batched snapshot of the provenance DAG."""
        async with self.database.read() as tx:
            requested = [await self.artifacts.get_in(tx, ref) for ref in refs]
            artifacts = (
                (
                    await tx.connection.execute(
                        sa.select(schema.artifacts.c.id, schema.artifacts.c.address)
                    )
                )
                .mappings()
                .all()
            )
            outputs = (
                (
                    await tx.connection.execute(
                        sa.select(schema.outputs.c.artifact_id, schema.outputs.c.result_id)
                    )
                )
                .mappings()
                .all()
            )
            inputs = (
                (
                    await tx.connection.execute(
                        sa.select(schema.inputs.c.result_id, schema.inputs.c.group)
                    )
                )
                .mappings()
                .all()
            )
        identities = {
            str(row["id"]): ArtifactRef(
                id=str(row["id"]),
                address=str(row["address"]),
                instance_id=self.artifacts.instance_id,
            )
            for row in artifacts
        }
        producers = {str(row["artifact_id"]): str(row["result_id"]) for row in outputs}
        parents: dict[str, set[str]] = {}
        for row in inputs:
            group = InputGroup.model_validate(row["group"])
            targets = parents.setdefault(str(row["result_id"]), set())
            for part in group.parts:
                targets.add(part.content.artifact.id)
                targets.update(origin.origin.artifact.id for origin in part.origins)
        memo: dict[str, set[str]] = {}

        def roots(identifier: str, visiting: frozenset[str] = frozenset()) -> set[str]:
            if identifier in memo:
                return memo[identifier]
            if identifier in visiting:
                raise NousError("content.provenance_cycle", "Content provenance contains a cycle")
            upstream = parents.get(producers.get(identifier, ""), set())
            found: set[str] = (
                set[str]().union(*(roots(parent, visiting | {identifier}) for parent in upstream))
                if upstream
                else {identifier}
            )
            memo[identifier] = found
            return found

        return {
            artifact.ref.id: tuple(identities[key] for key in sorted(roots(artifact.ref.id)))
            for artifact in requested
        }
