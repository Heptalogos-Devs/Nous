"""Streaming SHA-256 content publication and transactional body references."""

from __future__ import annotations

import asyncio
import errno
import hashlib
import os
import shutil
import tempfile
from collections.abc import AsyncGenerator, AsyncIterable, Callable, Iterable, Sequence
from contextlib import asynccontextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO

import sqlalchemy as sa
from sqlalchemy.dialects.sqlite import insert

from nous.common.errors import NousError
from nous.runtime.execution import ExecutionManager

from .content_schema import blobs, references
from .content_types import BlobRef
from .database import ReadTx, WriteTx


@dataclass(frozen=True)
class PreparedContent:
    path: Path
    blob: BlobRef


@dataclass(frozen=True)
class PublishedContent:
    blob: BlobRef


class ContentReader:
    """A bounded view over one immutable blob, without materializing the range."""

    def __init__(self, stream: BinaryIO, remaining: int) -> None:
        self._stream, self._remaining = stream, remaining

    def read(self, size: int = -1) -> bytes:
        length = self._remaining if size < 0 else min(size, self._remaining)
        data = self._stream.read(length)
        self._remaining -= len(data)
        return data


class ContentStore:
    """Own immutable bytes and authoritative owner/slot-to-blob associations.

    Publish bytes before opening the owning write transaction, then attach the
    published value in that transaction. Publication alone does not create an
    owner reference. The runtime owns the execution manager used for file work.
    """

    def __init__(
        self,
        root: Path,
        temporary: Path,
        execution: ExecutionManager,
        *,
        io_chunk_bytes: int = 1048576,
    ) -> None:
        self.root = root
        self.temporary = temporary / "content"
        self.execution = execution
        self.io_chunk_bytes = io_chunk_bytes
        self._publish_lock = asyncio.Lock()

    def path_for(self, blob: BlobRef) -> Path:
        return self.root / "objects" / blob.digest[:2] / blob.digest[2:]

    async def prepare(self, stream: AsyncIterable[bytes] | Iterable[bytes]) -> PreparedContent:
        """Hash and flush a stream to a temporary file for one publication.

        The iterable may be synchronous or asynchronous and may produce empty
        content. Publication consumes the returned temporary path; do not reuse
        the prepared value after attempting publication. A failed preparation
        removes its temporary file.

        Returns:
            The temporary path and computed digest and byte length.
        """

        def create() -> tuple[int, str]:
            self.temporary.mkdir(parents=True, exist_ok=True)
            return tempfile.mkstemp(dir=self.temporary, suffix=".part")

        descriptor, name = await self.execution.blocking(create)
        path = Path(name)
        digest = hashlib.sha256()
        length = 0
        try:
            with os.fdopen(descriptor, "wb") as output:

                def write(chunk: bytes) -> None:
                    output.write(chunk)
                    digest.update(chunk)

                if isinstance(stream, AsyncIterable):
                    async for chunk in stream:
                        await self._file_work(write, chunk)
                        length += len(chunk)
                else:
                    for chunk in stream:
                        await self._file_work(write, chunk)
                        length += len(chunk)

                def flush() -> None:
                    output.flush()
                    os.fsync(output.fileno())

                await self._file_work(flush)
            return PreparedContent(path, BlobRef(digest=digest.hexdigest(), byte_length=length))
        except BaseException:
            await self.execution.blocking(path.unlink, missing_ok=True)
            raise

    async def _file_work[T](self, action: Callable[..., T], *args: object) -> T:
        return await self.execution.blocking(action, *args)

    def _publish(self, prepared: PreparedContent) -> PublishedContent:
        """Make complete bytes visible at their digest path and consume temporary files."""
        target = self.path_for(prepared.blob)
        staging: Path | None = None
        try:
            target.parent.mkdir(parents=True, exist_ok=True)
            if target.exists():
                self._verify(target, prepared.blob)
            else:
                try:
                    os.replace(prepared.path, target)
                except OSError as exc:
                    if exc.errno != errno.EXDEV:
                        raise
                    # Stage on the content filesystem so the final rename remains atomic.
                    directory = self.root / "staging"
                    directory.mkdir(parents=True, exist_ok=True)
                    fd, name = tempfile.mkstemp(dir=directory, suffix=".part")
                    staging = Path(name)
                    with prepared.path.open("rb") as source, os.fdopen(fd, "wb") as output:
                        shutil.copyfileobj(source, output, length=self.io_chunk_bytes)
                        output.flush()
                        os.fsync(output.fileno())
                    self._verify(staging, prepared.blob)
                    os.replace(staging, target)
                if os.name == "posix":
                    descriptor = os.open(target.parent, os.O_RDONLY)
                    try:
                        os.fsync(descriptor)
                    finally:
                        os.close(descriptor)
            return PublishedContent(prepared.blob)
        finally:
            prepared.path.unlink(missing_ok=True)
            if staging is not None:
                staging.unlink(missing_ok=True)

    @staticmethod
    def _verify(path: Path, blob: BlobRef) -> None:
        if path.stat().st_size != blob.byte_length:
            raise NousError("content.corrupt", "Stored content has a conflicting byte length")
        with path.open("rb") as stream:
            if hashlib.file_digest(stream, "sha256").hexdigest() != blob.digest:
                raise NousError("content.corrupt", "Stored content does not match its digest")

    async def publish(self, prepared: PreparedContent) -> PublishedContent:
        """Publish prepared bytes under their digest and consume the temporary file.

        Existing bytes at that digest are verified. This operation does not
        attach a database reference; use ``attach_in`` in the owner's transaction.
        Returns:
            The published content identity for later attachment.

        Raises:
            NousError: ``content.corrupt`` if an existing object has the same
                digest path but conflicting bytes or length.
        """
        started = False
        try:
            async with self._publish_lock:
                started = True
                return await self.execution.blocking(self._publish, prepared)
        finally:
            if not started:
                await self.execution.blocking(prepared.path.unlink, missing_ok=True)

    async def publish_bytes(self, data: bytes) -> PublishedContent:
        return await self.publish(await self.prepare((data,)))

    async def publish_file(self, path: Path) -> PublishedContent:
        async def chunks() -> AsyncGenerator[bytes]:
            with path.open("rb") as source:
                while block := await self._file_work(source.read, self.io_chunk_bytes):
                    yield block

        stream = chunks()
        try:
            return await self.publish(await self.prepare(stream))
        finally:
            await stream.aclose()

    async def attach_in(
        self, tx: WriteTx, owner_ref: str, slot: str, published: PublishedContent
    ) -> BlobRef:
        """Attach published bytes to an owner and slot in the supplied transaction.

        The caller commits or rolls back the transaction along with the owning
        record. Publication must have completed first.

        Returns:
            The attached blob identity; it is committed with the transaction.

        Raises:
            NousError: ``content.missing`` if published bytes are unavailable,
                or ``content.corrupt`` if the stored length conflicts.
        """
        blob = published.blob
        if not self.path_for(blob).is_file():
            raise NousError("content.missing", "Published content is unavailable")
        await tx.connection.execute(
            insert(blobs).values(**blob.model_dump()).on_conflict_do_nothing()
        )
        length = await tx.connection.scalar(
            sa.select(blobs.c.byte_length).where(blobs.c.digest == blob.digest)
        )
        if length != blob.byte_length:
            raise NousError("content.corrupt", "Content identity has a conflicting length")
        await tx.connection.execute(
            references.insert().values(owner_ref=owner_ref, slot=slot, digest=blob.digest)
        )
        return blob

    async def detach_in(self, tx: WriteTx, owner_ref: str, slot: str) -> None:
        await tx.connection.execute(
            references.delete().where(
                references.c.owner_ref == owner_ref, references.c.slot == slot
            )
        )

    async def resolve_in(self, tx: ReadTx, owner_ref: str, slot: str = "body") -> BlobRef:
        return (await self.resolve_many_in(tx, (owner_ref,), slot))[0]

    async def resolve_many_in(
        self, tx: ReadTx, owner_refs: Sequence[str], slot: str = "body"
    ) -> tuple[BlobRef, ...]:
        """Resolve owner/slot bindings in the supplied transaction.

        Results follow ``owner_refs`` including duplicates; an empty input yields
        an empty tuple. If any requested binding is absent, raise ``NousError``
        with code ``content.missing`` instead of returning partial results.
        """
        found: dict[str, BlobRef] = {}
        identifiers = list(dict.fromkeys(owner_refs))
        for offset in range(0, len(identifiers), 800):
            rows = (
                await tx.connection.execute(
                    sa.select(blobs, references.c.owner_ref)
                    .join(references)
                    .where(
                        references.c.owner_ref.in_(identifiers[offset : offset + 800]),
                        references.c.slot == slot,
                    )
                )
            ).mappings()
            for row in rows:
                found[str(row["owner_ref"])] = BlobRef.model_validate(
                    {key: value for key, value in row.items() if key != "owner_ref"}
                )
        if any(identifier not in found for identifier in owner_refs):
            raise NousError("content.missing", "No content is attached to this record")
        return tuple(found[identifier] for identifier in owner_refs)

    @asynccontextmanager
    async def borrow_local(self, blob: BlobRef) -> AsyncGenerator[Path]:
        """Borrow a local immutable object path for the context lifetime.

        Checks existence and byte length before yielding; it does not rehash the
        file. The borrower must not modify the file or retain the path for work
        extending beyond the owning operation.

        Yields:
            The local path while the borrower uses the immutable object.

        Raises:
            NousError: ``content.missing`` if the file is absent or its length
                differs from the blob identity.
        """
        path = self.path_for(blob)

        def check() -> None:
            if not path.is_file() or path.stat().st_size != blob.byte_length:
                raise NousError("content.missing", "Stored content is missing or incomplete")

        await self.execution.blocking(check)
        yield path

    @asynccontextmanager
    async def open(
        self, blob: BlobRef, *, start: int = 0, end: int | None = None
    ) -> AsyncGenerator[ContentReader]:
        """Borrow a reader for a half-open byte range of an immutable object.

        Args:
            blob: Content identity and expected byte length.
            start: Inclusive byte offset, defaulting to zero.
            end: Exclusive byte offset; None selects through the end.

        Yields:
            A reader bounded to the requested range and closed on context exit.

        Raises:
            NousError: The range is invalid or the stored object is missing or
                has a different length. The reader closes on context exit.
        """
        boundary = blob.byte_length if end is None else end
        if not 0 <= start <= boundary <= blob.byte_length:
            raise NousError("content.invalid_range", "Byte selection exceeds the content")
        path = self.path_for(blob)
        if not path.is_file() or path.stat().st_size != blob.byte_length:
            raise NousError("content.missing", "Stored content is missing or incomplete")
        with path.open("rb") as stream:
            stream.seek(start)
            yield ContentReader(stream, boundary - start)

    async def read_bytes(self, blob: BlobRef, *, max_bytes: int = 64 * 1024 * 1024) -> bytes:
        if blob.byte_length > max_bytes:
            raise NousError(
                "content.too_large",
                "Content exceeds the read limit; use export",
                details={"byte_length": blob.byte_length, "max_bytes": max_bytes},
            )

        def read() -> bytes:
            path = self.path_for(blob)
            if not path.is_file() or path.stat().st_size != blob.byte_length:
                raise NousError("content.missing", "Stored content is missing or incomplete")
            return path.read_bytes()

        return await self.execution.blocking(read)

    async def read_text(self, blob: BlobRef, max_bytes: int = 8388608) -> str:
        try:
            return (await self.read_bytes(blob, max_bytes=max_bytes)).decode("utf-8")
        except UnicodeDecodeError as exc:
            raise NousError("content.not_text", "Content is not UTF-8 text; use export") from exc

    async def export(self, blob: BlobRef, destination: Path) -> Path:
        """Create and verify a copy at destination, returning that path.

        The destination must not already exist. A failed copy removes the file
        created by this call when possible; parent directories may remain.
        """

        def copy() -> Path:
            destination.parent.mkdir(parents=True, exist_ok=True)
            created = False
            try:
                with self.path_for(blob).open("rb") as source, destination.open("xb") as output:
                    created = True
                    shutil.copyfileobj(source, output, length=self.io_chunk_bytes)
                # Verify the delivered bytes, including mutations during the copy.
                self._verify(destination, blob)
                return destination
            except BaseException as error:
                if created:
                    try:
                        destination.unlink(missing_ok=True)
                    except OSError as cleanup:
                        error.add_note(
                            f"Failed export could not be removed: {destination}: {cleanup}"
                        )
                raise

        return await self._file_work(copy)
