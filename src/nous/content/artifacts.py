"""Artifact identities, exact body reading and content borrowing."""

from __future__ import annotations

import mimetypes
from collections.abc import AsyncGenerator, Callable, Sequence
from contextlib import asynccontextmanager
from datetime import datetime
from pathlib import Path
from typing import cast
from uuid import uuid4

import sqlalchemy as sa

from nous.common.errors import NousError
from nous.common.operations import record_completed
from nous.storage.content import ContentStore, PublishedContent
from nous.storage.database import Database, ReadTx, WriteTx
from nous.temporal.values import Clock

from . import schema
from ._records import date_value, insert_address
from .configuration import ContentConfig
from .contracts import (
    Artifact,
    ArtifactRef,
    BlobRef,
    ContentSelection,
    InputSpec,
    ResultRef,
    SourceSelection,
)
from .text import PARSER_PROFILE, BlockRef, TextBlock, TextView


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
        record_completed(artifact.ref.address)
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
        address = await insert_address(
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
            received_at=date_value(row["received_at"]),
            formed_at=date_value(row["formed_at"]),
            recorded_at=cast(datetime, date_value(row["recorded_at"])),
            producer_result_ref=producer,
        )

    async def blob(self, ref: str | ArtifactRef) -> BlobRef:
        if isinstance(ref, ArtifactRef):
            if ref.instance_id != self.instance_id:
                raise NousError("content.instance_mismatch", "Artifact belongs to another instance")
            target = ref.id
        else:
            target = ref
        async with self.database.read() as tx:
            identifier = await tx.connection.scalar(
                sa.select(schema.artifacts.c.id).where(
                    sa.or_(schema.artifacts.c.id == target, schema.artifacts.c.address == target)
                )
            )
            if identifier is None:
                raise NousError(
                    "content.not_found", "Artifact does not exist", details={"ref": target}
                )
            return await self.content.resolve_in(tx, str(identifier))

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
            with path.open("rb") as stream:
                stream.seek(start)
                data = stream.read(end - start)
            if len(data) != end - start:
                raise NousError("content.corrupt", "Stored content is incomplete")
            return data, start, 1
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

    async def _selected(self, content: ContentSelection) -> tuple[str, int, int]:
        blob = await self.blob(content.artifact)
        async with self.content.borrow_local(blob) as path:
            data, offset, line = await self.content.execution.blocking(
                self._selected_bytes,
                path,
                content.selection,
                self.text_read_max_bytes(),
                blob.byte_length,
            )
        try:
            return data.decode("utf-8"), offset, line
        except UnicodeDecodeError as error:
            raise NousError(
                "content.not_text", "Selection does not contain complete UTF-8 text"
            ) from error

    async def read_selection(self, content: ContentSelection) -> str:
        """Read only the requested bytes or streamed lines from immutable content."""
        return (await self._selected(content))[0]

    async def text_view(self, ref: ContentSelection | ArtifactRef | str) -> TextView:
        body = (
            ref
            if isinstance(ref, ContentSelection)
            else ContentSelection(artifact=(await self.get(ref)).ref)
        )
        text, offset, line = await self._selected(body)
        return await self.content.execution.blocking(
            TextView.parse, body, text, byte_offset=offset, first_line=line
        )

    async def read_blocks(
        self, refs: Sequence[BlockRef], *, preserve_context: bool = True
    ) -> tuple[TextBlock, ...]:
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
                raise NousError("content.block_mismatch", "Block does not match its immutable body")
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
