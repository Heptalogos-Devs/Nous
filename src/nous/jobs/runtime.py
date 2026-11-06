"""Own DBOS client, scheduler lifecycle and the shared SQLite connection pool."""

from __future__ import annotations

import asyncio
import logging
import threading
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager

from dbos import DBOS, DBOSClient, DBOSConfig, DBOSPortableJSONSerializer
from sqlalchemy import Engine
from sqlalchemy.exc import SQLAlchemyError

from nous.common.errors import NousError
from nous.runtime.execution import ExecutionManager, complete_cleanup
from nous.storage.database import Database

from .bridge import RuntimeBridge
from .configuration import JobsConfig
from .maintenance import CONTRACT_VERSION, MaintenanceStages, NousMaintenance
from .operations import QUEUES, JobOperations, application_name

_host_lock = threading.Lock()
_instances: dict[str, NousMaintenance] = {}


class _SchedulerLogFilter(logging.Filter):
    """Business diagnostics already own failures; do not log SDK frame contents again."""

    def filter(self, record: logging.LogRecord) -> bool:
        if record.exc_info is not None:
            record.error_type = type(record.exc_info[1]).__name__
            record.exc_info = None
            record.exc_text = None
            record.stack_info = None
        return True


def _register_queues(client: DBOSClient, settings: JobsConfig) -> None:
    for name, concurrency in (
        (QUEUES["annotation"], settings.annotation_concurrency),
        (QUEUES["refresh"], settings.refresh_concurrency),
    ):
        client.register_queue(
            name,
            worker_concurrency=concurrency,
            polling_interval_sec=settings.poll_interval_seconds,
            on_conflict="always_update",
        )


async def initialize_jobs(database: Database, instance_id: str, settings: JobsConfig) -> None:
    """Run the official system migration while the instance initialization lock is held."""

    async def initialize() -> None:
        engine = database.sync_engine(pool_size=settings.database_pool_size)
        try:
            url = database.migration_url()
            await asyncio.to_thread(DBOS.migrate, url)
            client = await asyncio.to_thread(
                DBOSClient,
                system_database_engine=engine,
                serializer=DBOSPortableJSONSerializer(),
                application_name=application_name(instance_id),
                retry_connection_errors=False,
            )
            try:
                await asyncio.to_thread(_register_queues, client, settings)
            finally:
                await asyncio.to_thread(client.destroy)
        finally:
            await asyncio.to_thread(engine.dispose)

    await complete_cleanup(initialize())


class JobRuntime:
    """The client may enqueue without workers; the host explicitly starts workers."""

    def __init__(
        self,
        client: DBOSClient,
        operations: JobOperations,
        database: Database,
        execution: ExecutionManager | None,
        settings: JobsConfig,
        engine: Engine,
    ) -> None:
        self._client = client
        self.operations = operations
        self.database = database
        self.execution = execution
        self.settings = settings
        self.engine = engine
        self.bridge: RuntimeBridge | None = None
        self.worker_running = False
        self._instance: NousMaintenance | None = None
        self._logging_state: tuple[list[logging.Handler], bool, int] | None = None
        self._log_filter = _SchedulerLogFilter()
        self._lifecycle_lock = asyncio.Lock()

    async def start(self, stages: MaintenanceStages) -> None:
        """Start the worker on DBOS's own loop with the bound business stages."""
        await complete_cleanup(self._start(stages))

    async def _start(self, stages: MaintenanceStages) -> None:
        async with self._lifecycle_lock:
            await self._launch(stages)

    async def _launch(self, stages: MaintenanceStages) -> None:
        if self.worker_running:
            raise NousError("job.host_in_use", "The durable worker is already running")
        if self.execution is None:
            raise NousError("job.host_unavailable", "A control client cannot start workers")
        bridge = RuntimeBridge(self.execution)
        self.bridge = bridge

        def launch() -> None:
            if not _host_lock.acquire(blocking=False):
                raise NousError(
                    "job.host_in_use", "A DBOS worker host is already running in this process"
                )
            constructed = False
            try:
                _register_queues(self._client, self.settings)
                dbos_logger = logging.getLogger("dbos")
                self._logging_state = (
                    list(dbos_logger.handlers),
                    dbos_logger.propagate,
                    dbos_logger.level,
                )
                dbos_logger.handlers = [logging.NullHandler()]
                dbos_logger.propagate = True
                dbos_logger.addFilter(self._log_filter)
                config: DBOSConfig = {
                    "name": self.operations.application,
                    "system_database_engine": self.engine,
                    "application_version": CONTRACT_VERSION,
                    "executor_id": self.operations.application,
                    "serializer": DBOSPortableJSONSerializer(),
                    "max_executor_threads": self.settings.max_executor_threads,
                    "run_migrations": False,
                    "enable_otlp": False,
                }
                constructed = True
                DBOS(config=config)
                dbos_logger.setLevel(logging.NOTSET)
                instance = _instances.get(self.operations.instance_id)
                if instance is None:
                    instance = NousMaintenance(self.operations.instance_id, bridge, stages)
                    _instances[self.operations.instance_id] = instance
                else:
                    instance.bind(bridge, stages)
                self._instance = instance
                DBOS.launch()
                self.worker_running = True
            except BaseException:
                try:
                    if constructed:
                        DBOS.destroy()
                finally:
                    if self._instance is not None:
                        self._instance.unbind()
                    self._restore_logging()
                    _host_lock.release()
                raise

        await asyncio.to_thread(launch)

    async def close(self, *, grace_seconds: float) -> None:
        """Stop dispatch, await real business cleanup and leave interrupted work recoverable."""
        await complete_cleanup(self._close(grace_seconds=grace_seconds))

    async def _close(self, *, grace_seconds: float) -> None:
        async with self._lifecycle_lock:
            if self.bridge is not None:
                await self.bridge.quiesce(grace_seconds=grace_seconds)
                self.bridge = None
            if self.worker_running:
                try:
                    await asyncio.to_thread(DBOS.destroy)
                finally:
                    if self._instance is not None:
                        self._instance.unbind()
                    self.worker_running = False
                    self._restore_logging()
                    _host_lock.release()

    def _restore_logging(self) -> None:
        if self._logging_state is not None:
            handlers, propagate, level = self._logging_state
            dbos_logger = logging.getLogger("dbos")
            dbos_logger.removeFilter(self._log_filter)
            dbos_logger.handlers = handlers
            dbos_logger.propagate = propagate
            dbos_logger.setLevel(level)
            self._logging_state = None


@asynccontextmanager
async def open_jobs(
    database: Database,
    instance_id: str,
    settings: JobsConfig,
    execution: ExecutionManager | None,
) -> AsyncGenerator[JobRuntime]:
    engine = database.sync_engine(pool_size=settings.database_pool_size)
    client: DBOSClient | None = None
    operations: JobOperations | None = None
    try:

        async def connect() -> None:
            nonlocal client
            client = await asyncio.to_thread(
                DBOSClient,
                system_database_engine=engine,
                serializer=DBOSPortableJSONSerializer(),
                application_name=application_name(instance_id),
                retry_connection_errors=False,
            )
            try:
                await asyncio.to_thread(
                    client.list_queues, application_name=application_name(instance_id)
                )
            except SQLAlchemyError as error:
                raise NousError(
                    "job.schema_missing",
                    "Durable job schema is unavailable; run instance initialization to upgrade.",
                ) from error

        await complete_cleanup(connect())
        assert client is not None
        operations = JobOperations(
            client,
            instance_id,
            settings,
            current_settings=(lambda: execution.snapshot.values.jobs) if execution else None,
        )
        yield JobRuntime(client, operations, database, execution, settings, engine)
    finally:

        async def cleanup() -> None:
            try:
                if operations is not None:
                    await operations.aclose()
                if client is not None:
                    await asyncio.to_thread(client.destroy)
            finally:
                await asyncio.to_thread(engine.dispose)

        await complete_cleanup(cleanup())
