"""Immutable streamed objects, publication leases, and transaction-owned references."""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import re
import shutil
from collections.abc import AsyncGenerator, AsyncIterable, Iterable
from contextlib import asynccontextmanager
from dataclasses import dataclass
from pathlib import Path
from uuid import UUID, uuid4

import anyio
import sqlalchemy as sa
from nous_wave.contracts.common import CallContext

from nous.platform.persistence import Authority, WriteTx, metadata

references = sa.Table(
    "nous_content_references",
    metadata,
    sa.Column("owner_ref", sa.String, primary_key=True),
    sa.Column("content_hash", sa.String, primary_key=True, index=True),
    sa.Column("namespace_id", sa.String, primary_key=True, index=True),
    sa.Column("byte_len", sa.Integer, nullable=False),
    sa.Column("media_type", sa.String, nullable=False),
    sa.Column("commit_seq", sa.Integer, nullable=False),
)


@dataclass(frozen=True)
class ContentObject:
    content_hash: str
    byte_len: int
    media_type: str


@dataclass(frozen=True)
class ContentLease(ContentObject):
    lease_id: str


class ContentStore:
    """Single-host store. The instance lock must be held before initialize().

    Publish outside SQL transactions, attach inside the owning domain transaction,
    then release the lease after that transaction has committed or rolled back.
    A lease protects bytes before they have a committed domain reference.
    """

    def __init__(
        self, authority: Authority, root: str | Path, *, max_bytes: int = 512 * 1024 * 1024
    ):
        self.authority = authority
        self.root = Path(root).resolve()
        self.objects = self.root / "objects"
        self.staging = self.root / "staging"
        self.leases = self.root / "leases"
        self.guards = self.root / "guards"
        self.max_bytes = max_bytes
        self._lock = asyncio.Lock()
        for path in (self.objects, self.staging, self.leases, self.guards):
            path.mkdir(parents=True, exist_ok=True)

    async def initialize(self) -> None:
        """Release abandoned leases and partial staging files from the previous host."""
        async with self._lock:
            for directory in (self.staging, self.leases, self.guards):
                for path in directory.iterdir():
                    if path.is_file():
                        path.unlink()
                    elif path.is_dir():
                        await anyio.to_thread.run_sync(shutil.rmtree, path)

    def path_for(self, content_hash: str) -> Path:
        if re.fullmatch(r"[a-f0-9]{64}", content_hash) is None:
            raise ValueError("Content identity must be a lowercase SHA-256 digest")
        return self.objects / content_hash[:2] / content_hash[2:]

    def _lease_path(self, lease_id: str) -> Path:
        return self.leases / str(UUID(lease_id))

    def _pin(self, content: ContentObject) -> ContentLease:
        lease = ContentLease(
            content.content_hash, content.byte_len, content.media_type, str(uuid4())
        )
        path = self._lease_path(lease.lease_id)
        with path.open("x", encoding="utf-8") as output:
            json.dump(
                {
                    "content_hash": content.content_hash,
                    "byte_len": content.byte_len,
                    "media_type": content.media_type,
                },
                output,
            )
            output.flush()
            os.fsync(output.fileno())
        return lease

    @staticmethod
    def _verify_existing(path: Path, digest: str, length: int) -> None:
        if path.stat().st_size != length:
            raise ValueError("Existing immutable content has a mismatched size")
        with path.open("rb") as source:
            if hashlib.file_digest(source, "sha256").hexdigest() != digest:
                raise ValueError("Existing immutable content failed its SHA-256 identity check")

    async def publish(self, chunks: AsyncIterable[bytes], media_type: str) -> ContentLease:
        if not media_type or "\n" in media_type or "\r" in media_type:
            raise ValueError("A valid media type is required")
        temporary = self.staging / str(uuid4())
        hasher = hashlib.sha256()
        length = 0
        try:
            async with await anyio.open_file(temporary, "xb") as output:
                async for chunk in chunks:
                    length += len(chunk)
                    if length > self.max_bytes:
                        raise ValueError("Content exceeds the instance publication size limit")
                    hasher.update(chunk)
                    await output.write(chunk)
                await output.flush()
                await anyio.to_thread.run_sync(os.fsync, output.wrapped.fileno())
            content = ContentObject(hasher.hexdigest(), length, media_type)
            target = self.path_for(content.content_hash)
            async with self._lock:
                target.parent.mkdir(parents=True, exist_ok=True)
                if target.exists():
                    await anyio.to_thread.run_sync(
                        self._verify_existing, target, content.content_hash, length
                    )
                else:
                    os.link(temporary, target)
                    target.chmod(0o444)
                    if os.name == "posix":
                        descriptor = os.open(target.parent, os.O_RDONLY)
                        try:
                            os.fsync(descriptor)
                        finally:
                            os.close(descriptor)
                return self._pin(content)
        finally:
            temporary.unlink(missing_ok=True)

    async def publish_bytes(self, value: bytes, media_type: str) -> ContentLease:
        async def chunks() -> AsyncGenerator[bytes]:
            yield value

        return await self.publish(chunks(), media_type)

    async def publish_file(self, path: str | Path, media_type: str) -> ContentLease:
        async def chunks() -> AsyncGenerator[bytes]:
            async with await anyio.open_file(path, "rb") as source:
                while block := await source.read(256 * 1024):
                    yield block

        return await self.publish(chunks(), media_type)

    async def iter_bytes(
        self, content: ContentObject, *, start: int = 0, end: int | None = None
    ) -> AsyncGenerator[bytes]:
        boundary = content.byte_len if end is None else end
        if start < 0 or boundary < start or boundary > content.byte_len:
            raise ValueError("Byte range is outside immutable content")
        async with self.protect(content) as path:
            async with await anyio.open_file(path, "rb") as source:
                await source.seek(start)
                remaining = boundary - start
                while remaining:
                    block = await source.read(min(256 * 1024, remaining))
                    if not block:
                        raise ValueError("Immutable content ended before its recorded length")
                    remaining -= len(block)
                    yield block

    @asynccontextmanager
    async def protect(self, content: ContentObject) -> AsyncGenerator[Path]:
        async with self._lock:
            path = self.path_for(content.content_hash)
            if path.stat().st_size != content.byte_len:
                raise ValueError("Content size differs from its authoritative reference")
            lease = self._pin(content)
        try:
            yield path
        finally:
            await self.release(lease)

    async def attach_in(self, tx: WriteTx, lease: ContentLease, owner_ref: str) -> ContentObject:
        if tx.authority is not self.authority:
            raise ValueError("Content reference and transaction use different authorities")
        async with self._lock:
            lease_path = self._lease_path(lease.lease_id)
            if (
                not lease_path.exists()
                or json.loads(lease_path.read_text())["content_hash"] != lease.content_hash
            ):
                raise ValueError("Publication lease is unavailable")
            if self.path_for(lease.content_hash).stat().st_size != lease.byte_len:
                raise ValueError("Published object does not match its reference")
            existing = await tx.connection.scalar(
                sa.select(references.c.owner_ref).where(
                    references.c.namespace_id == tx.ctx.namespace_id,
                    references.c.owner_ref == owner_ref,
                    references.c.content_hash == lease.content_hash,
                )
            )
            if existing is None:
                await tx.connection.execute(
                    references.insert().values(
                        owner_ref=owner_ref,
                        content_hash=lease.content_hash,
                        namespace_id=tx.ctx.namespace_id,
                        byte_len=lease.byte_len,
                        media_type=lease.media_type,
                        commit_seq=tx.commit_seq,
                    )
                )
        return ContentObject(lease.content_hash, lease.byte_len, lease.media_type)

    async def detach_in(self, tx: WriteTx, owner_ref: str) -> tuple[str, ...]:
        if tx.authority is not self.authority:
            raise ValueError("Content reference and transaction use different authorities")
        values = await tx.connection.execute(
            references.delete()
            .where(
                references.c.owner_ref == owner_ref,
                references.c.namespace_id == tx.ctx.namespace_id,
            )
            .returning(references.c.content_hash)
        )
        return tuple(values.scalars())

    async def release(self, lease: ContentLease) -> None:
        async with self._lock:
            self._lease_path(lease.lease_id).unlink(missing_ok=True)

    @asynccontextmanager
    async def preserve_all(self) -> AsyncGenerator[None]:
        """Pin all objects while an online snapshot enumerates and copies its refs.

        Publication and domain transactions remain available during the copy.
        """
        guard = self.guards / str(uuid4())
        async with self._lock:
            guard.touch(exist_ok=False)
        try:
            yield
        finally:
            async with self._lock:
                guard.unlink(missing_ok=True)

    async def purge(self, ctx: CallContext, content_hashes: Iterable[str]) -> tuple[str, ...]:
        hashes = tuple(dict.fromkeys(content_hashes))
        for digest in hashes:
            self.path_for(digest)
        removed: list[str] = []
        # Serialize deletion with attach_in: committed references cannot race unlink.
        async with self.authority.write(
            ctx, str(uuid4()), operation_kind="content.purge", scope="content", payload=hashes
        ) as tx:
            async with self._lock:
                if any(self.guards.iterdir()):
                    tx.result({"removed": removed})
                    return ()
                pinned = {
                    json.loads(path.read_text())["content_hash"]
                    for path in self.leases.iterdir()
                    if path.is_file()
                }
                retained = set(
                    (
                        await tx.connection.execute(
                            sa.select(references.c.content_hash).where(
                                references.c.content_hash.in_(hashes)
                            )
                        )
                    ).scalars()
                )
                for digest in hashes:
                    if digest not in retained and digest not in pinned:
                        path = self.path_for(digest)
                        if path.exists():
                            if os.name == "nt":
                                path.chmod(0o600)
                            path.unlink()
                            removed.append(digest)
            tx.result({"removed": removed})
        return tuple(removed)

    async def purge_unreferenced(self, ctx: CallContext) -> tuple[str, ...]:
        hashes = (
            path.parent.name + path.name for path in self.objects.glob("*/*") if path.is_file()
        )
        return await self.purge(ctx, hashes)

    async def referenced_objects(self) -> tuple[ContentObject, ...]:
        """Enumerate against this authority, which may be a restored backup snapshot."""
        async with self.authority.read() as view:
            rows = (
                await view.connection.execute(
                    sa.select(
                        references.c.content_hash, references.c.byte_len, references.c.media_type
                    ).distinct()
                )
            ).all()
        return tuple(ContentObject(row[0], row[1], row[2]) for row in rows)
