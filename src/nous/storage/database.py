"""SQLite connection policy and the instance's short transactions."""

from __future__ import annotations

import asyncio
import logging
import os
import sqlite3
import time
from collections.abc import AsyncGenerator, Generator
from contextlib import asynccontextmanager, closing, contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol
from uuid import uuid4

import sqlalchemy as sa
from sqlalchemy.engine import URL
from sqlalchemy.exc import OperationalError, SQLAlchemyError
from sqlalchemy.ext.asyncio import AsyncConnection, AsyncEngine, create_async_engine

from nous.observability import get_meter, get_tracer
from nous.runtime.errors import NousError

logger = logging.getLogger(__name__)

metadata = sa.MetaData()
instance_metadata = sa.Table(
    "instance_metadata",
    metadata,
    sa.Column("instance_id", sa.String, primary_key=True),
    sa.Column("database_id", sa.String, nullable=False),
    sa.Column("schema_version", sa.Integer, nullable=False),
)
time_domain_definitions = sa.Table(
    "time_domain_definitions",
    metadata,
    sa.Column("domain_id", sa.String, primary_key=True),
    sa.Column("kind", sa.String, nullable=False),
    sa.Column("name", sa.String, nullable=False),
    sa.Column("tick_numerator", sa.String),
    sa.Column("tick_denominator", sa.String),
    sa.Column("epoch", sa.String),
)
SCHEMA_VERSION = 1


@contextmanager
def _observe_transaction(mode: str) -> Generator[None]:
    meter = get_meter(__name__)
    duration = meter.create_histogram("database.transaction.duration", unit="s")
    errors = meter.create_counter("database.error.count")
    started = time.monotonic()
    try:
        yield
    except BaseException as exc:
        category = (
            "cancelled"
            if isinstance(exc, asyncio.CancelledError)
            else "database"
            if isinstance(exc, SQLAlchemyError)
            else "application"
        )
        errors.add(1, {"transaction": mode, "category": category})
        raise
    finally:
        duration.record(time.monotonic() - started, {"transaction": mode})


class DatabaseSettings(Protocol):
    @property
    def busy_timeout_ms(self) -> int: ...

    @property
    def synchronous(self) -> str: ...


@dataclass(frozen=True, slots=True)
class ReadTx:
    connection: AsyncConnection


@dataclass(frozen=True, slots=True)
class WriteTx(ReadTx):
    pass


@dataclass(frozen=True, slots=True)
class DatabaseStatus:
    instance_id: str
    database_id: str
    schema_version: int
    journal_mode: str
    foreign_keys: bool


def _setup_file(path: Path, *, create: bool) -> None:
    # A normal open must never create an empty replacement database.
    if create:
        try:
            fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        except FileExistsError as exc:
            raise NousError(
                "storage.exists", "Database already exists", details={"path": str(path)}
            ) from exc
        else:
            os.close(fd)
    if path.is_file() and path.stat().st_nlink > 1:
        raise NousError(
            "storage.invalid_path",
            "SQLite database hard links are unsupported.",
            details={"path": str(path)},
        )
    try:
        with closing(sqlite3.connect(path.as_uri() + "?mode=rw", uri=True, autocommit=True)) as db:
            if create:
                mode = str(db.execute("PRAGMA journal_mode=WAL").fetchone()[0]).lower()
            else:
                mode = str(db.execute("PRAGMA journal_mode").fetchone()[0]).lower()
            if mode != "wal":
                raise NousError(
                    "storage.invalid_format",
                    "Database is not in WAL mode",
                    details={"path": str(path), "journal_mode": mode},
                )
    except sqlite3.Error as exc:
        code = "storage.missing" if not path.is_file() else "storage.unavailable"
        raise NousError(code, "Cannot open instance database", details={"path": str(path)}) from exc


def _connection_factory(settings: DatabaseSettings) -> type[sqlite3.Connection]:
    """Apply connection PRAGMAs before Python opens its non-legacy transaction.

    sqlite3's documented factory runs on aiosqlite's connection worker.  Its
    public autocommit attribute lets us enable foreign keys outside a transaction
    without combining driver transaction control with manual BEGIN hooks.
    """
    timeout = int(settings.busy_timeout_ms)
    synchronous = str(settings.synchronous).upper()
    if timeout < 0 or synchronous not in {"FULL", "NORMAL"}:
        raise NousError("configuration.invalid_value", "Invalid database settings")

    class ConfiguredConnection(sqlite3.Connection):
        def __init__(
            self,
            database: str | bytes | os.PathLike[str] | os.PathLike[bytes],
            *,
            timeout: float = 5.0,
            detect_types: int = 0,
            isolation_level: str | None = None,
            check_same_thread: bool = True,
            factory: type[sqlite3.Connection] | None = None,
            cached_statements: int = 128,
            uri: bool = False,
            autocommit: bool = False,
        ) -> None:
            # isolation_level has no effect with autocommit=False.
            super().__init__(
                database,
                timeout=timeout,
                detect_types=detect_types,
                check_same_thread=check_same_thread,
                factory=factory,
                cached_statements=cached_statements,
                uri=uri,
                autocommit=True,
            )
            try:
                self.execute("PRAGMA foreign_keys=ON")
                self.execute(f"PRAGMA busy_timeout={timeout}")
                self.execute(f"PRAGMA synchronous={synchronous}")
                if self.execute("PRAGMA foreign_keys").fetchone() != (1,):
                    raise sqlite3.OperationalError("foreign keys could not be enabled")
                self.autocommit = False
            except BaseException:
                self.close()
                raise

    return ConfiguredConnection


class Database:
    def __init__(self, engine: AsyncEngine, instance_id: str, database_id: str | None) -> None:
        self._engine = engine
        self._instance_id = instance_id
        self._database_id = database_id
        self._writer_lock = asyncio.Lock()

    @classmethod
    @asynccontextmanager
    async def open(
        cls,
        path: Path,
        settings: DatabaseSettings,
        instance_id: str,
        *,
        create: bool = False,
        database_id: str | None = None,
    ) -> AsyncGenerator[Database]:
        path = await asyncio.to_thread(path.resolve)
        await asyncio.to_thread(_setup_file, path, create=create)
        url = URL.create(
            "sqlite+aiosqlite",
            database=path.as_uri(),
            query={"mode": "rw", "uri": "true"},
        )
        engine = create_async_engine(
            url,
            connect_args={"autocommit": False, "factory": _connection_factory(settings)},
        )
        database = cls(engine, instance_id, database_id)
        try:
            try:
                if create:
                    async with engine.begin() as connection:
                        await connection.run_sync(metadata.create_all)
                        await connection.execute(
                            instance_metadata.insert().values(
                                instance_id=instance_id,
                                database_id=database_id or str(uuid4()),
                                schema_version=SCHEMA_VERSION,
                            )
                        )
                await database.check()
            except SQLAlchemyError as exc:
                raise NousError(
                    "storage.unavailable",
                    "Cannot access instance database",
                    details={"path": str(path)},
                ) from exc
            logger.info("Instance database opened", extra={"database_path": str(path)})
            yield database
        finally:
            await engine.dispose()

    @asynccontextmanager
    async def read(self) -> AsyncGenerator[ReadTx]:
        with (
            get_tracer(__name__).start_as_current_span("database.read"),
            _observe_transaction("read"),
        ):
            async with self._engine.connect() as connection, connection.begin():
                yield ReadTx(connection)

    @asynccontextmanager
    async def write(self) -> AsyncGenerator[WriteTx]:
        with (
            get_tracer(__name__).start_as_current_span("database.write"),
            _observe_transaction("write"),
        ):
            async with self._writer_lock:
                try:
                    async with self._engine.begin() as connection:
                        yield WriteTx(connection)
                except OperationalError as exc:
                    if "locked" in str(exc).lower() or "busy" in str(exc).lower():
                        raise NousError("storage.busy", "Database is busy", retryable=True) from exc
                    raise

    async def check(self) -> DatabaseStatus:
        try:
            async with self.read() as tx:
                row = (await tx.connection.execute(sa.select(instance_metadata))).one_or_none()
                if row is None or row.instance_id != self._instance_id:
                    raise NousError(
                        "storage.instance_mismatch", "Database does not belong to this instance"
                    )
                if row.schema_version != SCHEMA_VERSION:
                    raise NousError("storage.invalid_format", "Unsupported database schema")
                if self._database_id is not None and row.database_id != self._database_id:
                    raise NousError(
                        "storage.binding_conflict",
                        "Database identity conflicts with the initialized instance resources.",
                    )
                mode = str(await tx.connection.scalar(sa.text("PRAGMA journal_mode"))).lower()
                foreign_keys = bool(await tx.connection.scalar(sa.text("PRAGMA foreign_keys")))
                if mode != "wal" or not foreign_keys:
                    raise NousError(
                        "storage.invalid_format", "SQLite connection policy is unavailable"
                    )
                return DatabaseStatus(
                    row.instance_id, row.database_id, row.schema_version, mode, foreign_keys
                )
        except OperationalError as exc:
            raise NousError(
                "storage.invalid_format", "Instance database schema is unreadable"
            ) from exc
