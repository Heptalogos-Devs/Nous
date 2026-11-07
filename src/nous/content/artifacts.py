"""Artifact identities, exact body reading and content borrowing."""

from __future__ import annotations

import codecs
import mimetypes
from collections.abc import AsyncGenerator, Callable, Mapping, Sequence
from contextlib import asynccontextmanager
from datetime import datetime
from pathlib import Path
from typing import Literal, cast
from uuid import uuid4

import sqlalchemy as sa
from pydantic import JsonValue

from nous.common.errors import NousError
from nous.common.identifier import insert_identifier
from nous.common.operations import record_completed
from nous.storage.content import ContentStore, PublishedContent
from nous.storage.database import Database, ReadTx, WriteTx
from nous.temporal.values import Clock

from . import schema
from ._records import date_value
from .configuration import ContentConfig, TextCacheConfig
from .contracts import (
    Artifact,
    ArtifactRef,
    BlobRef,
    ContentReadView,
    ContentRepresentation,
    ContentSelection,
    InputSpec,
    OriginSlice,
    ResultRef,
    SelectedText,
    SourceSelection,
)
from .text import PARSER_PROFILE, BlockRef, TextBlock, TextView


class ArtifactStore:
    """Own instance-bound artifact identities and their content access.

    Each acceptance records a separate artifact identity even when the same bytes
    already exist in CAS. Publish bytes before registering an artifact in a
    caller-owned transaction; the runtime owns the borrowed database and store.
    """

    def __init__(
        self,
        database: Database,
        content: ContentStore,
        instance_id: str,
        *,
        text_read_max_bytes: Callable[[], int] | None = None,
        text_cache_root: Path | None = None,
        text_cache_settings: Callable[[], TextCacheConfig] | None = None,
        clock: Clock | None = None,
    ) -> None:
        self.database, self.content, self.instance_id = database, content, instance_id
        self.clock = clock or Clock()
        from .text_cache import TextStructureCache

        self.text_cache = TextStructureCache(text_cache_root, text_cache_settings)
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
        """Accept a file, UTF-8 string or bytes as a new artifact.

        A ``Path`` reads a file; a string is text content, not a file path. Bytes
        are published first and the new identity and body reference are then
        committed in a short transaction. Repeated acceptance creates distinct
        artifact identities even if the bytes are identical.
        """
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
        record_completed(artifact.ref.identifier)
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
        object_id, recorded = str(uuid4()), self.clock.utc_now()
        identifier = await insert_identifier(
            tx,
            schema.artifacts,
            "art",
            {
                "id": object_id,
                "media_type": media_type,
                "display_name": display_name,
                "source": source,
                "received_at": received_at.isoformat() if received_at else None,
                "formed_at": formed_at.isoformat() if formed_at else None,
                "recorded_at": recorded.isoformat(),
            },
        )
        await self.content.attach_in(tx, object_id, "body", published)
        return Artifact(
            ref=ArtifactRef(id=object_id, identifier=identifier, instance_id=self.instance_id),
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
        return (await self.get_many_in(tx, (ref,)))[0]

    async def get_many(self, refs: Sequence[str | ArtifactRef]) -> tuple[Artifact, ...]:
        async with self.database.read() as tx:
            return await self.get_many_in(tx, refs)

    async def get_many_in(
        self, tx: ReadTx, refs: Sequence[str | ArtifactRef]
    ) -> tuple[Artifact, ...]:
        """Resolve artifacts in the caller's read transaction, preserving input order.

        String references can be IDs or identifiers; typed references must belong
        to this instance. Duplicates are preserved. A missing artifact or a
        reference from another instance raises ``NousError``.
        """
        targets: list[str] = []
        for ref in refs:
            if isinstance(ref, ArtifactRef):
                if ref.instance_id != self.instance_id:
                    raise NousError(
                        "content.instance_mismatch", "Artifact belongs to another instance"
                    )
                targets.append(ref.id)
            else:
                targets.append(ref)
        found: dict[str, Artifact] = {}
        object_ids = list(dict.fromkeys(targets))
        for offset in range(0, len(object_ids), 400):
            chunk = object_ids[offset : offset + 400]
            rows = (
                await tx.connection.execute(
                    sa.select(
                        schema.artifacts,
                        schema.results.c.id.label("producer_id"),
                        schema.results.c.identifier.label("producer_identifier"),
                    )
                    .outerjoin(
                        schema.outputs, schema.outputs.c.artifact_id == schema.artifacts.c.id
                    )
                    .outerjoin(schema.results, schema.results.c.id == schema.outputs.c.result_id)
                    .where(
                        sa.or_(
                            schema.artifacts.c.id.in_(chunk),
                            schema.artifacts.c.identifier.in_(chunk),
                        )
                    )
                )
            ).mappings()
            for row in rows:
                artifact = Artifact(
                    ref=ArtifactRef(
                        id=str(row["id"]),
                        identifier=str(row["identifier"]),
                        instance_id=self.instance_id,
                    ),
                    media_type=str(row["media_type"]),
                    display_name=str(row["display_name"]),
                    source=row["source"],
                    received_at=date_value(row["received_at"]),
                    formed_at=date_value(row["formed_at"]),
                    recorded_at=cast(datetime, date_value(row["recorded_at"])),
                    producer_result_ref=ResultRef(
                        id=str(row["producer_id"]),
                        identifier=str(row["producer_identifier"]),
                        instance_id=self.instance_id,
                    )
                    if row["producer_id"]
                    else None,
                )
                found[artifact.ref.id] = found[artifact.ref.identifier] = artifact
        missing = next((target for target in targets if target not in found), None)
        if missing is not None:
            raise NousError(
                "content.not_found", "Artifact does not exist", details={"ref": missing}
            )
        return tuple(found[target] for target in targets)

    async def blob(self, ref: str | ArtifactRef) -> BlobRef:
        if isinstance(ref, ArtifactRef):
            if ref.instance_id != self.instance_id:
                raise NousError("content.instance_mismatch", "Artifact belongs to another instance")
            target = ref.id
        else:
            target = ref
        async with self.database.read() as tx:
            object_id = await tx.connection.scalar(
                sa.select(schema.artifacts.c.id).where(
                    sa.or_(schema.artifacts.c.id == target, schema.artifacts.c.identifier == target)
                )
            )
            if object_id is None:
                raise NousError(
                    "content.not_found", "Artifact does not exist", details={"ref": target}
                )
            return await self.content.resolve_in(tx, str(object_id))

    async def read_view_many(
        self,
        selections: Sequence[ContentSelection],
        *,
        representations: Literal["existing", "none"] = "existing",
    ) -> tuple[ContentReadView, ...]:
        """Read exact text or media references, expanding adopted representations without generation."""
        async with self.content.execution.operation("content.read_view_many"):
            if not selections:
                return ()
            async with self.database.read() as tx:
                originals = await self.get_many_in(
                    tx, [selection.artifact for selection in selections]
                )
                blobs = await self.content.resolve_many_in(tx, [item.ref.id for item in originals])
                object_ids = {item.ref.id for item in originals}
                represented: dict[
                    str, list[tuple[dict[str, object], tuple[ContentSelection, ...]]]
                ] = {}
                if representations == "existing":
                    direct = sa.select(schema.parts.c.result_id).where(
                        schema.parts.c.artifact_id.in_(object_ids)
                    )
                    origins = sa.select(schema.origins.c.result_id).where(
                        schema.origins.c.artifact_id.in_(object_ids)
                    )
                    result_ids = list(
                        (await tx.connection.execute(direct.union(origins))).scalars()
                    )
                    if result_ids:
                        used = (
                            (
                                await tx.connection.execute(
                                    sa.select(
                                        schema.parts.c.result_id,
                                        schema.parts.c.artifact_id,
                                        schema.parts.c.selector,
                                    ).where(schema.parts.c.result_id.in_(result_ids))
                                )
                            )
                            .mappings()
                            .all()
                        )
                        roots = (
                            (
                                await tx.connection.execute(
                                    sa.select(
                                        schema.origins.c.result_id,
                                        schema.origins.c.artifact_id,
                                        schema.origins.c.selector,
                                    ).where(schema.origins.c.result_id.in_(result_ids))
                                )
                            )
                            .mappings()
                            .all()
                        )
                        source_artifacts = await self.get_many_in(
                            tx, [str(row["artifact_id"]) for row in (*used, *roots)]
                        )
                        relations: dict[str, list[ContentSelection]] = {}
                        for row, artifact in zip((*used, *roots), source_artifacts, strict=True):
                            selected = ContentSelection(
                                artifact=artifact.ref,
                                selection=SourceSelection.model_validate(row["selector"]),
                            )
                            if selected not in relations.setdefault(str(row["result_id"]), []):
                                relations[str(row["result_id"])].append(selected)
                        outputs = (
                            (
                                await tx.connection.execute(
                                    sa.select(
                                        schema.results.c.id,
                                        schema.results.c.identifier,
                                        schema.results.c.operation,
                                        schema.results.c.coverage,
                                        schema.results.c.parameters,
                                        schema.outputs.c.name,
                                        schema.outputs.c.artifact_id,
                                    )
                                    .join(schema.outputs)
                                    .where(
                                        schema.results.c.id.in_(result_ids),
                                        schema.outputs.c.name != "response",
                                    )
                                    .order_by(
                                        schema.results.c.recorded_at, schema.outputs.c.ordinal
                                    )
                                )
                            )
                            .mappings()
                            .all()
                        )
                        for row in outputs:
                            declared = cast(dict[str, JsonValue], row["parameters"]).get(
                                "representations", {}
                            )
                            info = (
                                declared.get(str(row["name"]))
                                if isinstance(declared, dict)
                                else None
                            )
                            if not isinstance(info, dict) or not isinstance(info.get("kind"), str):
                                continue
                            exact = (
                                tuple(
                                    ContentSelection.model_validate(item)
                                    for item in cast(list[JsonValue], info["sources"])
                                )
                                if "sources" in info
                                else tuple(relations[str(row["id"])])
                            )
                            if any(item.artifact.instance_id != self.instance_id for item in exact):
                                raise NousError(
                                    "content.instance_mismatch",
                                    "A representation belongs to another instance",
                                )
                            details: dict[str, object] = {
                                **dict(row),
                                "representation_kind": info["kind"],
                            }
                            for object_id in {item.artifact.id for item in exact} & object_ids:
                                represented.setdefault(object_id, []).append((details, exact))
                derived = await self.get_many_in(
                    tx,
                    list(
                        dict.fromkeys(
                            str(row["artifact_id"])
                            for rows in represented.values()
                            for row, _ in rows
                        )
                    ),
                )
            derived_map = {artifact.ref.id: artifact for artifact in derived}
            texts: dict[str, str] = {}
            views: list[ContentReadView] = []
            for selected, artifact, blob in zip(selections, originals, blobs, strict=True):
                is_text = (
                    artifact.media_type.startswith("text/")
                    or artifact.media_type == "application/json"
                )
                async with self.content.borrow_local(blob) as path:
                    if is_text:
                        data, _, _ = await self.content.execution.blocking(
                            self._selected_bytes,
                            path,
                            selected.selection,
                            self.text_read_max_bytes(),
                            blob.byte_length,
                        )
                        try:
                            body = data.decode("utf-8")
                        except UnicodeDecodeError as error:
                            raise NousError(
                                "content.not_text", "Selection does not contain complete UTF-8 text"
                            ) from error
                    else:
                        body = None
                available: list[ContentRepresentation] = []
                normalized_selected = (
                    (await self.normalize_selections((selected,)))[0] if is_text else selected
                )
                for row, exact in represented.get(artifact.ref.id, []):
                    normalized_exact = await self.normalize_selections(exact)
                    if not any(
                        item.artifact.id == artifact.ref.id
                        and self._overlaps(normalized_selected.selection, item.selection)
                        for item in normalized_exact
                    ):
                        continue
                    output = derived_map[str(row["artifact_id"])]
                    if output.ref.id == artifact.ref.id:
                        continue
                    if (
                        output.media_type.startswith("text/")
                        or output.media_type == "application/json"
                    ):
                        if output.ref.id not in texts:
                            texts[output.ref.id] = await self.read_text(output.ref)
                        text = texts[output.ref.id]
                    else:
                        text = None
                    available.append(
                        ContentRepresentation(
                            kind=str(row["representation_kind"]),
                            result_ref=ResultRef(
                                id=str(row["id"]),
                                identifier=str(row["identifier"]),
                                instance_id=self.instance_id,
                            ),
                            operation=str(row["operation"]),
                            output_name=str(row["name"]),
                            content=ContentSelection(artifact=output.ref),
                            name=output.display_name,
                            media_type=output.media_type,
                            source_selections=exact,
                            coverage=cast(dict[str, JsonValue], row["coverage"]),
                            body_text=text,
                        )
                    )
                views.append(
                    ContentReadView(
                        source=selected,
                        name=artifact.display_name,
                        media_type=artifact.media_type,
                        byte_length=blob.byte_length,
                        body_text=body,
                        export_ref=artifact.ref,
                        available_representations=tuple(available),
                        coverage={"selection": selected.selection.model_dump(mode="json")},
                    )
                )
            return tuple(views)

    @staticmethod
    def _overlaps(left: SourceSelection, right: SourceSelection) -> bool:
        if left.is_whole or right.is_whole:
            return True
        if left.pages or right.pages:
            return bool(set(left.pages) & set(right.pages))
        for low, high, inclusive in (
            ("start_byte", "end_byte", False),
            ("start_line", "end_line", True),
            ("start_ms", "end_ms", False),
        ):
            if any(
                getattr(item, low) is not None or getattr(item, high) is not None
                for item in (left, right)
            ):
                if not all(
                    getattr(item, low) is not None or getattr(item, high) is not None
                    for item in (left, right)
                ):
                    return False
                start = max(getattr(left, low) or 0, getattr(right, low) or 0)
                end = min(
                    getattr(left, high) if getattr(left, high) is not None else float("inf"),
                    getattr(right, high) if getattr(right, high) is not None else float("inf"),
                )
                if not (start <= end if inclusive else start < end):
                    return False
        if left.frame is not None and right.frame is not None and left.frame != right.frame:
            return False
        if left.crop is not None and right.crop is not None:
            return max(left.crop[0], right.crop[0]) < min(left.crop[2], right.crop[2]) and max(
                left.crop[1], right.crop[1]
            ) < min(left.crop[3], right.crop[3])
        return True

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

    @staticmethod
    def _selected_bytes(
        path: Path, selection: SourceSelection, limit: int, byte_length: int
    ) -> tuple[bytes, int, int]:
        if selection.pages or any(
            value is not None
            for value in (selection.start_ms, selection.end_ms, selection.frame, selection.crop)
        ):
            raise NousError(
                "content.selection_requires_preparation",
                "Media coordinates require media preparation",
            )
        if selection.start_byte is not None or selection.end_byte is not None:
            start, end = (
                selection.start_byte or 0,
                selection.end_byte if selection.end_byte is not None else byte_length,
            )
            if not 0 <= start <= end <= byte_length:
                raise NousError("content.invalid_selection", "Byte selection exceeds the artifact")
            if end - start > limit:
                raise NousError(
                    "content.too_large", "Text selection exceeds the configured read limit"
                )
            first_line = 1
            previous_cr = False
            decoder = codecs.getincrementaldecoder("utf-8")()
            with path.open("rb") as stream:
                remaining = start
                try:
                    while remaining:
                        prefix = stream.read(min(65536, remaining))
                        if not prefix:
                            raise NousError("content.corrupt", "Stored content is incomplete")
                        remaining -= len(prefix)
                        decoded = decoder.decode(prefix, final=remaining == 0)
                        for character in decoded:
                            if character == "\r":
                                first_line += 1
                            elif character == "\n" and not previous_cr:
                                first_line += 1
                            previous_cr = character == "\r"
                except UnicodeDecodeError as error:
                    raise NousError(
                        "content.invalid_selection", "Selection splits a UTF-8 character"
                    ) from error
                data = stream.read(end - start)
                try:
                    data.decode("utf-8")
                except UnicodeDecodeError as error:
                    raise NousError(
                        "content.invalid_selection", "Selection splits a UTF-8 character"
                    ) from error
            if len(data) != end - start:
                raise NousError("content.corrupt", "Stored content is incomplete")
            return data, start, first_line
        if selection.start_line is None and selection.end_line is None:
            if byte_length > limit:
                raise NousError(
                    "content.too_large", "Text exceeds the configured read limit; select a range"
                )
            return path.read_bytes(), 0, 1
        first, last = selection.start_line or 1, selection.end_line
        parts: list[bytes] = []
        offset = start = size = count = 0
        try:
            with path.open("r", encoding="utf-8", newline="") as stream:
                for number, line in enumerate(stream, 1):
                    count = number
                    data = line.encode("utf-8")
                    if number < first:
                        offset += len(data)
                        continue
                    if number == first:
                        start = offset
                    size += len(data)
                    if size > limit:
                        raise NousError(
                            "content.too_large", "Text selection exceeds the configured read limit"
                        )
                    parts.append(data)
                    offset += len(data)
                    if last is not None and number == last:
                        break
        except UnicodeDecodeError as error:
            raise NousError("content.not_text", "Stored content is not UTF-8 text") from error
        if first > count or last is not None and last > count:
            raise NousError("content.invalid_selection", "Text selection exceeds the artifact")
        return b"".join(parts), start, first

    async def read_selections(self, values: Sequence[ContentSelection]) -> tuple[SelectedText, ...]:
        """Read exact bounded text ranges in order using one metadata snapshot."""
        async with self.content.execution.operation("content.read_selections"):
            async with self.database.read() as tx:
                originals = await self.get_many_in(tx, [value.artifact for value in values])
                blobs = await self.content.resolve_many_in(tx, [item.ref.id for item in originals])
            result: list[SelectedText] = []
            for value, blob in zip(values, blobs, strict=True):
                async with self.content.borrow_local(blob) as path:
                    data, start, line = await self.content.execution.blocking(
                        self._selected_bytes,
                        path,
                        value.selection,
                        self.text_read_max_bytes(),
                        blob.byte_length,
                    )
                try:
                    text = data.decode("utf-8")
                except UnicodeDecodeError as error:
                    raise NousError(
                        "content.not_text", "Selection does not contain complete UTF-8 text"
                    ) from error
                content = ContentSelection(
                    artifact=value.artifact,
                    selection=SourceSelection(start_byte=start, end_byte=start + len(data)),
                )
                result.append(
                    SelectedText(
                        content=content,
                        blob=blob,
                        data=data,
                        text=text,
                        start_byte=start,
                        end_byte=start + len(data),
                        first_line=line,
                    )
                )
            return tuple(result)

    @staticmethod
    def _normalize_text(
        path: Path, selection: SourceSelection, byte_length: int
    ) -> SourceSelection:
        """Resolve coordinates without retaining or limiting unrequested source bytes."""
        if selection.pages or any(
            value is not None
            for value in (selection.start_ms, selection.end_ms, selection.frame, selection.crop)
        ):
            raise NousError("content.invalid_selection", "Text requires byte or line coordinates")
        if selection.start_line is None and selection.end_line is None:
            start = selection.start_byte or 0
            end = selection.end_byte if selection.end_byte is not None else byte_length
            if not 0 <= start <= end <= byte_length:
                raise NousError("content.invalid_selection", "Byte selection exceeds the artifact")
            with path.open("rb") as stream:
                for position in (start, end):
                    if position < byte_length:
                        stream.seek(position)
                        character = stream.read(1)
                        if not character:
                            raise NousError("content.corrupt", "Stored content is incomplete")
                        if character[0] & 0xC0 == 0x80:
                            raise NousError(
                                "content.invalid_selection", "Selection splits a UTF-8 character"
                            )
            return SourceSelection(start_byte=start, end_byte=end)
        first, last = selection.start_line or 1, selection.end_line
        count = offset = start = 0
        try:
            with path.open("r", encoding="utf-8", newline="") as stream:
                for count, line in enumerate(stream, 1):
                    if count == first:
                        start = offset
                    offset += len(line.encode("utf-8"))
                    if last is not None and count == last:
                        break
        except UnicodeDecodeError as error:
            raise NousError("content.not_text", "Stored content is not UTF-8 text") from error
        if first > count or last is not None and last > count:
            raise NousError("content.invalid_selection", "Text selection exceeds the artifact")
        return SourceSelection(start_byte=start, end_byte=offset)

    async def normalize_selections(
        self, values: Sequence[ContentSelection], *, media_types: Mapping[str, str] | None = None
    ) -> tuple[ContentSelection, ...]:
        """Canonicalize text coordinates; preserve media coordinates for its format owner."""
        async with self.content.execution.operation("content.normalize_selections"):
            async with self.database.read() as tx:
                originals = await self.get_many_in(tx, [value.artifact for value in values])
                blobs = await self.content.resolve_many_in(tx, [item.ref.id for item in originals])
            normalized: list[ContentSelection] = []
            for value, artifact, blob in zip(values, originals, blobs, strict=True):
                media_type = (media_types or {}).get(artifact.ref.id, artifact.media_type)
                media_coordinates = bool(value.selection.pages) or any(
                    item is not None
                    for item in (
                        value.selection.start_ms,
                        value.selection.end_ms,
                        value.selection.frame,
                        value.selection.crop,
                    )
                )
                if not media_coordinates and (
                    media_type.startswith("text/")
                    or media_type
                    in {
                        "application/json",
                        "application/xml",
                        "application/xhtml+xml",
                    }
                ):
                    async with self.content.borrow_local(blob) as path:
                        selected = await self.content.execution.blocking(
                            self._normalize_text, path, value.selection, blob.byte_length
                        )
                    value = ContentSelection(artifact=value.artifact, selection=selected)
                normalized.append(value)
            return tuple(normalized)

    async def map_origins(
        self, content: ContentSelection, target: ArtifactRef
    ) -> tuple[OriginSlice, ...]:
        """Map an adopted representation to exact target bytes using its declared coordinate maps."""
        from .mappings import map_origins

        async with self.content.execution.operation("content.map_origins"):
            return await map_origins(self, content, target)

    async def read_selection(self, content: ContentSelection) -> str:
        return (await self.read_selections((content,)))[0].text

    async def text_views(self, values: Sequence[ContentSelection]) -> tuple[TextView, ...]:
        async with self.content.execution.operation("content.text_views"):
            result: list[TextView] = []
            for item in await self.read_selections(values):
                if item.start_byte == 0 and item.end_byte == item.blob.byte_length:
                    view, _ = await self.text_cache.view(
                        item.content.artifact, item.blob, item.text, self.content.execution
                    )
                else:
                    view = await self.content.execution.blocking(
                        TextView.parse,
                        item.content,
                        item.text,
                        byte_offset=item.start_byte,
                        first_line=item.first_line,
                    )
                result.append(view)
            return tuple(result)

    async def text_view(self, ref: ContentSelection | ArtifactRef | str) -> TextView:
        body = (
            ref
            if isinstance(ref, ContentSelection)
            else ContentSelection(artifact=(await self.get(ref)).ref)
        )
        return (await self.text_views((body,)))[0]

    async def read_blocks(
        self, refs: Sequence[BlockRef], *, preserve_context: bool = True
    ) -> tuple[TextBlock, ...]:
        """Resolve and verify exact block references against their original body views.

        Returns first occurrences in input order, deduplicated by body and block
        key. Unknown parser profiles, blocks or mismatched byte selections raise
        ``NousError``; references must match the body and view that produced them.
        """
        async with self.content.execution.operation("content.read_blocks"):
            views: dict[str, TextView] = {}
            selected: list[TextBlock] = []
            seen: set[tuple[str, str]] = set()
            for ref in refs:
                if ref.parser_profile != PARSER_PROFILE:
                    raise NousError("content.parser_profile_unknown", "Unknown text parser profile")
                body = ref.view or ContentSelection(artifact=ref.body)
                view_key = body.model_dump_json()
                view = views.get(view_key)
                if view is None:
                    view = await self.text_view(body)
                    views[view_key] = view
                block = view.block(ref.key)
                if block.selection != ref.content:
                    raise NousError(
                        "content.block_mismatch", "Block does not match its immutable body"
                    )
                identity = (ref.body.id, ref.key)
                if identity not in seen:
                    selected.append(block)
                    seen.add(identity)
            return tuple(selected)

    @asynccontextmanager
    async def borrow_local(self, ref: str | ArtifactRef) -> AsyncGenerator[Path]:
        async with self.content.borrow_local(await self.blob(ref)) as path:
            yield path

    async def export(self, ref: str | ArtifactRef, destination: Path) -> Path:
        return await self.content.export(await self.blob(ref), destination)
