"""Restricted job control without acquiring the business instance lock."""

from __future__ import annotations

from collections.abc import AsyncGenerator, Mapping
from contextlib import asynccontextmanager
from pathlib import Path

from nous.configuration import Configuration
from nous.runtime.bindings import InstanceBinding
from nous.runtime.instance import InstanceDescriptor, InstanceLocator
from nous.runtime.paths import ResolvedPaths
from nous.runtime.schema import BUSINESS_METADATA
from nous.storage.database import Database

from .operations import JobOperations
from .runtime import open_jobs


@asynccontextmanager
async def open_job_client(
    locator: InstanceLocator | Path | str | None = None,
    *,
    environment: Mapping[str, str] | None = None,
) -> AsyncGenerator[JobOperations]:
    """Check the existing binding and expose only the DBOS management operations."""
    selected = InstanceLocator.resolve(locator, environment)
    configuration = Configuration(selected.config_file, environment=environment)
    settings = configuration.snapshot.values
    descriptor = InstanceDescriptor(
        settings.instance.id, settings.instance.name, settings.instance.layout
    )
    paths = ResolvedPaths.resolve(selected, descriptor, settings.paths)
    binding = InstanceBinding.read(selected)
    binding.check_locations(selected, descriptor, paths, settings.database.filename)
    binding.check_resources(paths)
    async with Database.open(
        paths.resolve_owned("data", settings.database.filename),
        settings.database,
        descriptor.instance_id,
        database_id=str(binding.database_id),
        additional_metadata=BUSINESS_METADATA,
    ) as database:
        async with open_jobs(database, descriptor.instance_id, settings.jobs, None) as host:
            yield host.operations
