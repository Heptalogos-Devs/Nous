"""DBOS client connections and ownership of an explicitly started host."""

from __future__ import annotations

import asyncio
import threading
from collections.abc import AsyncGenerator, Sequence
from contextlib import asynccontextmanager
from typing import TYPE_CHECKING

import dbos._dbos as dbos_runtime
from dbos import DBOS, DBOSClient, DBOSConfig, DBOSPortableJSONSerializer
from sqlalchemy import Engine
from sqlalchemy.exc import SQLAlchemyError

from nous.common.errors import NousError
from nous.runtime.execution import ExecutionManager, complete_cleanup
from nous.storage.database import Database

from .configuration import JobsConfig
from .contracts import QueueDefinition
from .operations import JobOperations, application_name

if TYPE_CHECKING:
    from nous.cognition.maintenance.operations import MaintenanceOperations

_host_lock = threading.Lock()


def _active_dbos() -> DBOS | None:
    return dbos_runtime._dbos_global_instance  # pyright: ignore[reportPrivateUsage]


def _registry_in_use() -> bool:
    """Check the pinned SDK registry without modifying another application's registrations."""
    registry = dbos_runtime._dbos_global_registry  # pyright: ignore[reportPrivateUsage]
    return registry is not None and bool(
        registry.workflow_info_map
        or registry.class_info_map
        or registry.instance_info_map
        or registry.pollers
        or registry.datasources
        or registry.kafka_registrations
    )


def _register_queues(
    client: DBOSClient, settings: JobsConfig, definitions: Sequence[QueueDefinition]
) -> None:
    for definition in definitions:
        client.register_queue(
            definition.queue,
            worker_concurrency=definition.concurrency,
            polling_interval_sec=settings.poll_interval_seconds,
            on_conflict="always_update",
        )


async def initialize_jobs(
    database: Database,
    instance_id: str,
    settings: JobsConfig,
    definitions: Sequence[QueueDefinition],
) -> None:
    """Run SDK migrations and register queues during explicit instance initialization."""

    async def initialize() -> None:
        engine = database.sync_engine(pool_size=settings.database_pool_size)
        try:
            await asyncio.to_thread(DBOS.migrate, database.migration_url())
            client = await asyncio.to_thread(
                DBOSClient,
                system_database_engine=engine,
                serializer=DBOSPortableJSONSerializer(),
                application_name=application_name(instance_id),
                retry_connection_errors=False,
            )
            try:
                await asyncio.to_thread(_register_queues, client, settings, definitions)
            finally:
                await asyncio.to_thread(client.destroy)
        finally:
            await asyncio.to_thread(engine.dispose)

    await complete_cleanup(initialize())


class JobRuntime:
    """Own the control client; a durable host alone may claim the DBOS singleton."""

    def __init__(
        self,
        client: DBOSClient,
        operations: JobOperations,
        execution: ExecutionManager,
        settings: JobsConfig,
        engine: Engine,
        definitions: Sequence[QueueDefinition],
    ) -> None:
        self._client = client
        self.operations = operations
        self.execution = execution
        self.settings = settings
        self.engine = engine
        self.definitions = tuple(definitions)
        self.worker_running = False
        self._dbos: DBOS | None = None

    async def start(self, maintenance: MaintenanceOperations) -> None:
        """Register and launch on the same running loop as the owning application."""
        if self.worker_running or not _host_lock.acquire(blocking=False):
            raise NousError("job.host_in_use", "A durable host is already running")
        owned: DBOS | None = None
        registered = False
        try:
            if _active_dbos() is not None or _registry_in_use():
                raise NousError("job.host_in_use", "DBOS is already owned by this process")
            await self.execution.control_blocking(
                _register_queues, self._client, self.settings, self.definitions
            )
            if _active_dbos() is not None or _registry_in_use():
                raise NousError("job.host_in_use", "DBOS is already owned by this process")
            from nous.cognition.maintenance.durable import register

            registered = True
            register(self.operations.instance_id, self.execution, maintenance)
            config: DBOSConfig = {
                "name": self.operations.application,
                "system_database_engine": self.engine,
                "executor_id": self.operations.application,
                "serializer": DBOSPortableJSONSerializer(),
                "max_executor_threads": self.settings.max_executor_threads,
                "run_migrations": False,
                "enable_otlp": False,
            }
            owned = DBOS(config=config)
            if _active_dbos() is not owned:
                raise NousError("job.host_in_use", "DBOS is already owned by this process")
            self._dbos = owned
            DBOS.launch()
            self.worker_running = True
        except BaseException:
            try:
                if registered and (_active_dbos() is owned or _active_dbos() is None):
                    await self.execution.control_blocking(DBOS.destroy, destroy_registry=True)
            finally:
                self._dbos = None
                _host_lock.release()
            raise

    async def close(self) -> None:
        """Destroy only this host's SDK after accepted business tasks have exited."""
        if not self.worker_running:
            return
        if self._dbos is None or _active_dbos() is not self._dbos:
            raise NousError("job.host_ownership_lost", "The DBOS worker is no longer owned")
        try:
            await complete_cleanup(
                self.execution.control_blocking(
                    DBOS.destroy, destroy_registry=True, workflow_completion_timeout_sec=30
                )
            )
        finally:
            self._dbos = None
            self.worker_running = False
            _host_lock.release()


@asynccontextmanager
async def open_jobs(
    database: Database,
    instance_id: str,
    settings: JobsConfig,
    execution: ExecutionManager,
    definitions: Sequence[QueueDefinition],
) -> AsyncGenerator[JobRuntime]:
    engine = database.sync_engine(pool_size=settings.database_pool_size)
    client: DBOSClient | None = None
    try:

        async def connect() -> None:
            nonlocal client
            client = await execution.control_blocking(
                DBOSClient,
                system_database_engine=engine,
                serializer=DBOSPortableJSONSerializer(),
                application_name=application_name(instance_id),
                retry_connection_errors=False,
            )
            try:
                await execution.control_blocking(
                    client.list_queues, application_name=application_name(instance_id)
                )
            except SQLAlchemyError as error:
                raise NousError(
                    "job.schema_missing",
                    "Durable job schema is unavailable; initialize the instance.",
                ) from error

        await complete_cleanup(connect())
        assert client is not None
        operations = JobOperations(
            client,
            instance_id,
            settings,
            execution,
            definitions,
            current_settings=lambda: execution.snapshot.values.jobs,
        )
        yield JobRuntime(client, operations, execution, settings, engine, definitions)
    finally:

        async def cleanup() -> None:
            try:
                if client is not None:
                    await execution.control_blocking(client.destroy)
            finally:
                await execution.control_blocking(engine.dispose)

        await complete_cleanup(cleanup())
