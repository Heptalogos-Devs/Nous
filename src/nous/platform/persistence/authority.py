"""Short, explicitly owned SQLite transactions and durable operation receipts."""

from __future__ import annotations

import hashlib
import json
import sqlite3
from collections.abc import AsyncGenerator, Callable
from contextlib import asynccontextmanager, closing
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import TYPE_CHECKING, Protocol, TypeVar

import sqlalchemy as sa
from nous_sdk.contracts.common import CallContext, DomainError
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import event
from sqlalchemy.dialects.sqlite import insert
from sqlalchemy.engine.interfaces import DBAPICursor
from sqlalchemy.ext.asyncio import AsyncConnection, create_async_engine
from sqlalchemy.pool import ConnectionPoolEntry

from nous.platform.concurrency import owned_thread
from nous.platform.observability import operation_span

from .schema import clock, metadata, operations

if TYPE_CHECKING:
    from nous.platform.execution import DurableExecution, JobRef, JobSpec

T = TypeVar("T")


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


def operation_key(ctx: CallContext, operation_kind: str, scope: str, operation_id: str) -> str:
    if not operation_id or not operation_kind:
        raise ValueError("operation_id and operation_kind are required")
    identity = (
        ctx.caller_id,
        operation_kind,
        ctx.namespace_id,
        ctx.subject_id,
        scope,
        operation_id,
    )
    return hashlib.sha256(canonical_json(identity).encode()).hexdigest()


class OperationConflict(ValueError):
    """An operation identity was reused with a different command."""


class OperationResultPurged(DomainError):
    def __init__(self, operation_id: str):
        super().__init__(
            "operation_result_purged",
            "The operation completed, and its source result was purged",
            operation_id=operation_id,
        )


def _require_retained_result(operation_id: str, value: JsonValue) -> JsonValue:
    if isinstance(value, dict) and value.get("$nous_operation_result") == "purged":
        raise OperationResultPurged(operation_id)
    return value


@dataclass(frozen=True)
class OperationReceipt:
    operation_id: str
    operation_kind: str
    commit_seq: int
    recorded_at: str
    result: JsonValue
    jobs: tuple[str, ...]


@dataclass(frozen=True)
class ReadView:
    connection: AsyncConnection
    commit_seq: int


class WriteTx:
    def __init__(
        self,
        connection: AsyncConnection,
        *,
        authority: Authority,
        ctx: CallContext,
        operation_id: str,
        operation_kind: str,
        scope: str,
        operation_key: str,
        payload_hash: str,
        commit_seq: int,
        recorded_at: str,
        receipt: OperationReceipt | None = None,
    ):
        self.connection = connection
        self.authority = authority
        self.ctx = ctx
        self.operation_id = operation_id
        self.operation_kind = operation_kind
        self.scope = scope
        self.operation_key = operation_key
        self.payload_hash = payload_hash
        self.commit_seq = commit_seq
        self.recorded_at = recorded_at
        self.receipt = receipt
        self.replayed = receipt is not None
        self.jobs: list[str] = []
        self.result_set = False
        self.result_value: JsonValue = None

    @property
    def replay_result(self) -> JsonValue:
        if not self.replayed or self.receipt is None:
            raise RuntimeError("Replay result requires a completed prior operation")
        return _require_retained_result(self.operation_id, self.receipt.result)

    def result(self, value: T) -> T:
        if self.replayed:
            raise RuntimeError("A replay must use the original receipt result")
        if self.result_set:
            raise RuntimeError("Transaction result was already recorded")
        # Validate serialization before any commit is allowed.
        self.result_value = json.loads(canonical_json(value))
        self.result_set = True
        self.receipt = OperationReceipt(
            self.operation_id,
            self.operation_kind,
            self.commit_seq,
            self.recorded_at,
            self.result_value,
            tuple(self.jobs),
        )
        return value

    async def enqueue(self, spec: JobSpec) -> JobRef:
        if self.replayed or self.result_set:
            raise RuntimeError("Jobs must be enqueued before setting the transaction result")
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

    def __init__(self, path: str | Path, *, busy_timeout_ms: int = 5000):
        self.path = Path(path).resolve()
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.execution: DurableExecution | None = None
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
    async def write(
        self,
        ctx: CallContext,
        operation_id: str,
        *,
        operation_kind: str,
        scope: str = "",
        payload: object,
    ) -> AsyncGenerator[WriteTx]:
        key = operation_key(ctx, operation_kind, scope, operation_id)
        payload_hash = hashlib.sha256(
            canonical_json({"kind": operation_kind, "payload": payload}).encode()
        ).hexdigest()
        with operation_span(operation_kind, ctx, operation_id=operation_id) as span:
            async with self.writer.connect() as connection, connection.begin():
                row = (
                    (
                        await connection.execute(
                            sa.select(operations).where(operations.c.operation_key == key)
                        )
                    )
                    .mappings()
                    .first()
                )
                if row:
                    if row["payload_hash"] != payload_hash:
                        raise OperationConflict(
                            f"Operation {operation_id!r} already has a different payload"
                        )
                    retained = _require_retained_result(
                        row["operation_id"], json.loads(row["result_json"])
                    )
                    receipt = OperationReceipt(
                        row["operation_id"],
                        row["operation_kind"],
                        row["commit_seq"],
                        row["recorded_at"],
                        retained,
                        tuple(json.loads(row["jobs_json"])),
                    )
                    tx = WriteTx(
                        connection,
                        authority=self,
                        ctx=ctx,
                        operation_id=operation_id,
                        operation_kind=operation_kind,
                        scope=scope,
                        operation_key=key,
                        payload_hash=payload_hash,
                        commit_seq=receipt.commit_seq,
                        recorded_at=receipt.recorded_at,
                        receipt=receipt,
                    )
                    # Prevent an accidental second execution from mutating domain facts.
                    await connection.exec_driver_sql("PRAGMA query_only=ON")
                    try:
                        yield tx
                    finally:
                        await connection.exec_driver_sql("PRAGMA query_only=OFF")
                else:
                    seq = await connection.scalar(
                        sa.update(clock)
                        .where(clock.c.singleton == 1)
                        .values(commit_seq=clock.c.commit_seq + 1)
                        .returning(clock.c.commit_seq)
                    )
                    if seq is None:
                        raise RuntimeError("Authority commit clock is missing")
                    now = datetime.now(UTC).isoformat()
                    tx = WriteTx(
                        connection,
                        authority=self,
                        ctx=ctx,
                        operation_id=operation_id,
                        operation_kind=operation_kind,
                        scope=scope,
                        operation_key=key,
                        payload_hash=payload_hash,
                        commit_seq=int(seq),
                        recorded_at=now,
                    )
                    yield tx
                    if not tx.result_set:
                        raise RuntimeError("A successful write must record tx.result(value)")
                    await connection.execute(
                        operations.insert().values(
                            operation_key=key,
                            caller_id=ctx.caller_id,
                            namespace_id=ctx.namespace_id,
                            subject_id=ctx.subject_id,
                            scope=scope,
                            operation_id=operation_id,
                            operation_kind=operation_kind,
                            payload_hash=payload_hash,
                            commit_seq=seq,
                            recorded_at=now,
                            result_json=canonical_json(tx.result_value),
                            jobs_json=canonical_json(tx.jobs),
                        )
                    )
            # A sequence becomes observable only after SQLite has committed.
            span.set_attribute("nous.replayed", tx.replayed)
            span.set_attribute("nous.commit_seq", tx.commit_seq)

    async def receipt(
        self,
        ctx: CallContext,
        operation_id: str,
        *,
        operation_kind: str,
        scope: str = "",
        payload: object,
    ) -> OperationReceipt | None:
        """Read a completed receipt before repeating expensive external work."""
        key = operation_key(ctx, operation_kind, scope, operation_id)
        expected = hashlib.sha256(
            canonical_json({"kind": operation_kind, "payload": payload}).encode()
        ).hexdigest()
        async with self.read(ctx) as view:
            row = (
                (
                    await view.connection.execute(
                        sa.select(operations).where(operations.c.operation_key == key)
                    )
                )
                .mappings()
                .first()
            )
            if row is None:
                return None
            if row["payload_hash"] != expected:
                raise OperationConflict(
                    f"Operation {operation_id!r} already has a different payload"
                )
            retained = _require_retained_result(row["operation_id"], json.loads(row["result_json"]))
            return OperationReceipt(
                row["operation_id"],
                row["operation_kind"],
                row["commit_seq"],
                row["recorded_at"],
                retained,
                tuple(json.loads(row["jobs_json"])),
            )

    async def redact_receipts_in(self, tx: WriteTx, predicate: Callable[[JsonValue], bool]) -> int:
        """Erase matching source-bearing results while retaining operation identity.

        The source owner supplies the domain predicate. A repeated command observes
        the tombstone and cannot repeat an already completed paid operation.
        """
        if tx.authority is not self:
            raise ValueError("Receipt purge must use this authority's transaction")
        rows = (
            await tx.connection.execute(
                sa.select(operations.c.operation_key, operations.c.result_json).where(
                    operations.c.namespace_id == tx.ctx.namespace_id
                )
            )
        ).all()
        changed = 0
        for key, raw in rows:
            value: JsonValue = json.loads(raw)
            if predicate(value):
                await tx.connection.execute(
                    operations.update()
                    .where(operations.c.operation_key == key)
                    .values(result_json=canonical_json({"$nous_operation_result": "purged"}))
                )
                changed += 1
        return changed

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
