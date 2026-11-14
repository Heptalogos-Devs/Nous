"""Artifact identities, exact body reading and content borrowing."""

from __future__ import annotations

import mimetypes
from collections.abc import AsyncGenerator, Callable, Mapping, Sequence
from contextlib import asynccontextmanager
from datetime import datetime
from pathlib import Path
from typing import Literal, cast
from uuid import uuid4

import sqlalchemy as sa
from PIL import Image, UnidentifiedImageError
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
    OriginSlice,
    ResultRef,
    SelectedText,
    SourceFacts,
    SourceSelection,
    is_text_media_type,
    is_utf8_text_prefix,
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
        record_completed(artifact.ref)
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
        resolved = tuple(found[target] for target in targets)
        if any(
            isinstance(ref, ArtifactRef) and ref != artifact.ref
            for ref, artifact in zip(refs, resolved, strict=True)
        ):
            raise NousError(
                "content.reference_mismatch", "Artifact reference identity does not match"
            )
        return resolved

    async def blob(self, ref: str | ArtifactRef) -> BlobRef:
        if isinstance(ref, ArtifactRef):
            if ref.instance_id != self.instance_id:
                raise NousError("content.instance_mismatch", "Artifact belongs to another instance")
            target = ref.id
        else:
            target = ref
        async with self.database.read() as tx:
            row = (
                await tx.connection.execute(
                    sa.select(schema.artifacts.c.id, schema.artifacts.c.identifier).where(
                        sa.or_(
                            schema.artifacts.c.id == target, schema.artifacts.c.identifier == target
                        )
                    )
                )
            ).one_or_none()
            if row is None:
                raise NousError(
                    "content.not_found", "Artifact does not exist", details={"ref": target}
                )
            if isinstance(ref, ArtifactRef) and ref.identifier != row.identifier:
                raise NousError(
                    "content.reference_mismatch", "Artifact reference identity does not match"
                )
            return await self.content.resolve_in(tx, str(row.id))

    async def _detected_types_in(self, tx: ReadTx, object_ids: set[str]) -> dict[str, str]:
        direct = sa.select(schema.parts.c.result_id).where(
            schema.parts.c.artifact_id.in_(object_ids)
        )
        origins = sa.select(schema.origins.c.result_id).where(
            schema.origins.c.artifact_id.in_(object_ids)
        )
        selected = sa.select(schema.selected_targets.c.result_id).where(
            schema.selected_targets.c.artifact_id.in_(object_ids)
        )
        result_ids = list((await tx.connection.execute(direct.union(origins, selected))).scalars())
        if not result_ids:
            return {}
        rows = await tx.connection.execute(
            sa.select(schema.results.c.selected_facts).where(schema.results.c.id.in_(result_ids))
        )
        detected: dict[str, str] = {}
        for row in rows.scalars():
            for value in row or ():
                fact = SourceFacts.model_validate(value)
                detected[fact.artifact.id] = fact.detected_mime
        return detected

    async def text_capabilities(self, selections: Sequence[ContentSelection]) -> tuple[bool, ...]:
        """Determine whether selected artifacts have readable text without loading their bodies."""
        if not selections:
            return ()
        async with self.database.read() as tx:
            artifacts = await self.get_many_in(tx, [item.artifact for item in selections])
            blobs = await self.content.resolve_many_in(tx, [item.ref.id for item in artifacts])
            detected = await self._detected_types_in(tx, {item.ref.id for item in artifacts})
        statuses: dict[str, bool] = {}
        for artifact, blob in zip(artifacts, blobs, strict=True):
            if artifact.ref.id in statuses:
                continue
            media_type = detected.get(artifact.ref.id)
            if media_type is not None:
                statuses[artifact.ref.id] = is_text_media_type(media_type)
            else:
                async with self.content.borrow_local(blob) as path:
                    statuses[artifact.ref.id] = await self.content.execution.blocking(
                        self._looks_like_text, path
                    )
        return tuple(statuses[item.ref.id] for item in artifacts)

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
                detected_types = await self._detected_types_in(tx, object_ids)
                if representations == "existing":
                    representation_ids = list(
                        (
                            await tx.connection.execute(
                                sa.select(schema.mapping_origins.c.result_id)
                                .select_from(
                                    schema.mapping_origins.join(
                                        schema.outputs,
                                        sa.and_(
                                            schema.mapping_origins.c.result_id
                                            == schema.outputs.c.result_id,
                                            schema.mapping_origins.c.output_name
                                            == schema.outputs.c.name,
                                        ),
                                    )
                                )
                                .where(
                                    schema.mapping_origins.c.artifact_id.in_(object_ids),
                                    schema.outputs.c.representation_kind.is_not(None),
                                    schema.outputs.c.role != "diagnostic",
                                )
                                .distinct()
                            )
                        ).scalars()
                    )
                    if representation_ids:
                        rows = (
                            (
                                await tx.connection.execute(
                                    sa.select(
                                        schema.results.c.id,
                                        schema.results.c.identifier,
                                        schema.results.c.operation,
                                        schema.results.c.coverage,
                                        schema.outputs.c.name,
                                        schema.outputs.c.artifact_id,
                                        schema.outputs.c.representation_kind,
                                        schema.mapping_origins.c.artifact_id.label("source_id"),
                                        schema.mapping_origins.c.selector,
                                    )
                                    .select_from(
                                        schema.results.join(schema.outputs).join(
                                            schema.mapping_origins,
                                            sa.and_(
                                                schema.mapping_origins.c.result_id
                                                == schema.outputs.c.result_id,
                                                schema.mapping_origins.c.output_name
                                                == schema.outputs.c.name,
                                            ),
                                        )
                                    )
                                    .where(
                                        schema.results.c.id.in_(representation_ids),
                                        schema.outputs.c.representation_kind.is_not(None),
                                        schema.outputs.c.role != "diagnostic",
                                    )
                                    .order_by(
                                        schema.results.c.recorded_at,
                                        schema.outputs.c.ordinal,
                                        schema.mapping_origins.c.mapping_ordinal,
                                        schema.mapping_origins.c.ordinal,
                                    )
                                )
                            )
                            .mappings()
                            .all()
                        )
                        source_artifacts = await self.get_many_in(
                            tx, [str(row["source_id"]) for row in rows]
                        )
                        relations: dict[
                            tuple[str, str], tuple[dict[str, object], list[ContentSelection]]
                        ] = {}
                        for row, source in zip(rows, source_artifacts, strict=True):
                            key = (str(row["id"]), str(row["name"]))
                            details, selected_sources = relations.setdefault(key, (dict(row), []))
                            origin = ContentSelection(
                                artifact=source.ref,
                                selection=SourceSelection.model_validate(row["selector"]),
                            )
                            if origin not in selected_sources:
                                selected_sources.append(origin)
                        for details, sources in relations.values():
                            exact = tuple(sources)
                            for object_id in {source.artifact.id for source in exact} & object_ids:
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
            cached_views: dict[str, ContentReadView] = {}
            normalized_relations: dict[tuple[str, ...], tuple[ContentSelection, ...]] = {}
            views: list[ContentReadView] = []
            for selected, artifact, blob in zip(selections, originals, blobs, strict=True):
                selection_key = selected.model_dump_json()
                if selection_key in cached_views:
                    views.append(cached_views[selection_key])
                    continue
                detected_type = detected_types.get(artifact.ref.id)
                async with self.content.borrow_local(blob) as path:
                    if detected_type is None:
                        is_text = await self.content.execution.blocking(self._looks_like_text, path)
                        if is_text and not is_text_media_type(artifact.media_type):
                            detected_type = "text/plain"
                    else:
                        is_text = is_text_media_type(detected_type)
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
                    (
                        await self.normalize_selections(
                            (selected,),
                            media_types={artifact.ref.id: detected_type or artifact.media_type},
                        )
                    )[0]
                    if is_text
                    else selected
                )
                for row, exact in represented.get(artifact.ref.id, []):
                    relation_key = tuple(item.model_dump_json() for item in exact)
                    if relation_key not in normalized_relations:
                        normalized_relations[relation_key] = await self.normalize_selections(
                            exact, media_types=detected_types
                        )
                    normalized_exact = normalized_relations[relation_key]
                    if not any(
                        item.artifact.id == artifact.ref.id
                        and self._overlaps(normalized_selected.selection, item.selection)
                        for item in normalized_exact
                    ):
                        continue
                    output = derived_map[str(row["artifact_id"])]
                    if output.ref.id == artifact.ref.id:
                        continue
                    if is_text_media_type(output.media_type):
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
                view = ContentReadView(
                    source=selected,
                    name=artifact.display_name,
                    media_type=artifact.media_type,
                    detected_media_type=detected_type,
                    byte_length=blob.byte_length,
                    body_text=body,
                    export_ref=artifact.ref,
                    available_representations=tuple(available),
                    coverage={"selection": selected.selection.model_dump(mode="json")},
                )
                cached_views[selection_key] = view
                views.append(view)
            return tuple(views)

    @staticmethod
    def _looks_like_text(path: Path) -> bool:
        with path.open("rb") as stream:
            header = stream.read(4096)
        if not is_utf8_text_prefix(header):
            return False
        try:
            with Image.open(path):
                return False
        except UnidentifiedImageError, OSError:
            return True

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

    @staticmethod
    def _selected_bytes(
        path: Path, selection: SourceSelection, limit: int, byte_length: int
    ) -> tuple[bytes, int, int]:
        from .text_selection import read_seekable

        with path.open("rb", buffering=0) as stream:
            return read_seekable(stream.fileno(), selection, limit)

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
        from .text_selection import normalize_seekable

        with path.open("rb", buffering=0) as stream:
            return normalize_seekable(stream.fileno(), selection, byte_length)

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
                if not media_coordinates and is_text_media_type(media_type):
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
