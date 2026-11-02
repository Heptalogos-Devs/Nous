"""Short, explicitly owned SQLite transactions and atomic durable work admission."""

from __future__ import annotations

import json
import sqlite3
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager, closing
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path
from typing import TYPE_CHECKING, Protocol

import sqlalchemy as sa
from nous_sdk.contracts.common import CallContext
from pydantic import TypeAdapter
from sqlalchemy import event
from sqlalchemy.dialects.sqlite import insert
from sqlalchemy.engine.interfaces import DBAPICursor
from sqlalchemy.ext.asyncio import AsyncConnection, create_async_engine
from sqlalchemy.pool import ConnectionPoolEntry

from nous.platform.concurrency import owned_thread
from nous.platform.observability import operation_span

from .schema import clock, metadata

if TYPE_CHECKING:
    from nous.platform.execution import DurableExecution, JobRef, JobSpec

    from .namespace_limits import NamespaceLimits




_json_encoder = TypeAdapter(object)


def canonical_json(value: object) -> str:
    return json.dumps(
        _json_encoder.dump_python(value, mode="json"),
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
        allow_nan=False,
    )


class SQLiteConnection(Protocol):
    isolation_level: str | None

    def cursor(self) -> DBAPICursor: ...


class OperationConflict(ValueError):
    """A business operation conflicts with the current authoritative state."""


@dataclass(frozen=True)
class ReadView:
    connection: AsyncConnection
    commit_seq: int
    facts: dict[object, object] = field(default_factory=dict, compare=False, repr=False)


class WriteTx:
    def __init__(
        self,
        connection: AsyncConnection,
        *,
        authority: Authority,
        ctx: CallContext,
        commit_seq: int,
        recorded_at: str,
    ):
        self.connection = connection
        self.authority = authority
        self.ctx = ctx
        self.commit_seq = commit_seq
        self.recorded_at = recorded_at
        self.jobs: list[str] = []
        self.storage_baseline: int | None = None

    async def enqueue(self, spec: JobSpec) -> JobRef:
        if self.authority.execution is None:
            raise RuntimeError("Durable execution was not attached to this authority")
        ref = await self.authority.execution.enqueue_in(self, spec)
        self.jobs.append(ref.workflow_id)
        return ref


class Authority:
    """One physical authority database, with separate reader and writer policies.

    Domain tables share metadata before initialize(). Workers start only after schema initialization.
    The context determines operation identity, not permissions: domains still enforce
    their own authorization in this transaction.
    """

    def __init__(
        self,
        path: str | Path,
        *,
        busy_timeout_ms: int = 5000,
        namespace_limits: NamespaceLimits | None = None,
    ):
        self.path = Path(path).resolve()
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.execution: DurableExecution | None = None
        self.namespace_limits = namespace_limits
        self.initialized = False
        url = sa.URL.create("sqlite+aiosqlite", database=str(self.path))
        self.reader = create_async_engine(url)
        self.writer = create_async_engine(url)
        for engine, writer in ((self.reader, False), (self.writer, True)):

            @event.listens_for(engine.sync_engine, "connect")
            def connect(
                dbapi_connection: SQLiteConnection,
                _record: ConnectionPoolEntry,
                *,
                readonly: bool = not writer,
            ) -> None:
                dbapi_connection.isolation_level = None
                cursor = dbapi_connection.cursor()
                cursor.execute("PRAGMA foreign_keys=ON")
                cursor.execute(f"PRAGMA busy_timeout={int(busy_timeout_ms)}")
                if readonly:
                    cursor.execute("PRAGMA query_only=ON")
                cursor.close()

            @event.listens_for(engine.sync_engine, "begin")
            def begin(connection: sa.Connection, *, immediate: bool = writer) -> None:
                connection.exec_driver_sql("BEGIN IMMEDIATE" if immediate else "BEGIN")

    async def initialize(self) -> None:
        async with self.writer.connect() as connection:
            # SQLite changes journal mode outside the application transaction.
            await connection.run_sync(self._enable_wal)
            async with connection.begin():
                await connection.run_sync(metadata.create_all, checkfirst=True)
                for table in metadata.sorted_tables:
                    for index in table.indexes:
                        await connection.run_sync(index.create, checkfirst=True)
                await connection.execute(
                    insert(clock)
                    .values(singleton=1, commit_seq=0)
                    .on_conflict_do_nothing(index_elements=[clock.c.singleton])
                )
        self.initialized = True

    @staticmethod
    def _enable_wal(connection: sa.Connection) -> None:
        raw = connection.connection.dbapi_connection
        if raw is None:
            raise RuntimeError("Schema initialization requires an open DBAPI connection")
        cursor = raw.cursor()
        try:
            cursor.execute("PRAGMA journal_mode=WAL")
        finally:
            cursor.close()

    @asynccontextmanager
    async def read(self, ctx: CallContext | None = None) -> AsyncGenerator[ReadView]:
        async with self.reader.connect() as connection, connection.begin():
            seq = await connection.scalar(
                sa.select(clock.c.commit_seq).where(clock.c.singleton == 1)
            )
            yield ReadView(connection, int(seq or 0))

    @asynccontextmanager
    async def write(self, ctx: CallContext) -> AsyncGenerator[WriteTx]:
        with operation_span("authority.write", ctx) as span:
            async with self.writer.connect() as connection, connection.begin():
                storage_before = None
                if self.namespace_limits is not None:
                    quota = self.namespace_limits.quota(ctx)
                    if quota.max_storage_bytes is not None:
                        usage = await self.namespace_limits.usage_in(ReadView(connection, 0), ctx)
                        storage_before = int(str(usage["storage_bytes"]))
                seq = await connection.scalar(
                    sa.update(clock)
                    .where(clock.c.singleton == 1)
                    .values(commit_seq=clock.c.commit_seq + 1)
                    .returning(clock.c.commit_seq)
                )
                if seq is None:
                    raise RuntimeError("Authority commit clock is missing")
                tx = WriteTx(
                    connection,
                    authority=self,
                    ctx=ctx,
                    commit_seq=int(seq),
                    recorded_at=datetime.now(UTC).isoformat(),
                )
                tx.storage_baseline = storage_before
                yield tx
                if tx.storage_baseline is not None and self.namespace_limits is not None:
                    await self.namespace_limits.enforce_in(tx, ctx, tx.storage_baseline)
            span.set_attribute("nous.commit_seq", tx.commit_seq)

    async def backup(self, destination: str | Path) -> Path:
        """Take a consistent online snapshot including DBOS and committed WAL data."""
        return await owned_thread(lambda: self._copy_database(self.path, Path(destination)))

    @staticmethod
    async def restore_backup(source: str | Path, destination: str | Path) -> Path:
        """Restore into a new instance path before opening its authority or workers."""
        return await owned_thread(lambda: Authority._copy_database(Path(source), Path(destination)))

    @staticmethod
    def _copy_database(source: Path, destination: Path) -> Path:
        source = source.resolve()
        destination = destination.resolve()
        destination.parent.mkdir(parents=True, exist_ok=True)
        # Exclusive creation preserves an existing instance, including an active WAL.
        with destination.open("xb"):
            pass
        try:
            with closing(sqlite3.connect(f"{source.as_uri()}?mode=ro", uri=True)) as current:
                with closing(sqlite3.connect(destination)) as snapshot:
                    current.backup(snapshot, pages=128)
                    if snapshot.execute("PRAGMA integrity_check").fetchone() != ("ok",):
                        raise ValueError("Database snapshot failed its integrity check")
        except BaseException:
            destination.unlink(missing_ok=True)
            raise
        return destination

    async def close(self) -> None:
        await self.reader.dispose()
        await self.writer.dispose()
