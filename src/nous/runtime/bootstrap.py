"""Construct and close exactly the resources used by a local instance."""

import logging
from collections.abc import AsyncGenerator, Mapping, Sequence
from contextlib import AsyncExitStack, ExitStack, asynccontextmanager
from dataclasses import dataclass
from pathlib import Path
from uuid import uuid4

import tomlkit

from nous.configuration import ConfigSnapshot, Configuration
from nous.observability import LoggingSession, Observability, operation
from nous.storage.database import Database
from nous.temporal import TimeService, bind_domains, check_domains

from .bindings import InstanceBinding, prepare_resource_directories
from .errors import NousError
from .execution import ExecutionManager
from .instance import InstanceDescriptor, InstanceLocator, InstanceLock
from .paths import ResolvedPaths

logger = logging.getLogger(__name__)


@asynccontextmanager
async def _resources() -> AsyncGenerator[AsyncExitStack]:
    """Keep the initiating failure when resource cleanup also reports a failure."""
    stack = AsyncExitStack()
    try:
        yield stack
    except BaseException as primary:
        try:
            await stack.aclose()
        except BaseException as cleanup:
            primary.add_note(f"Resource cleanup also failed: {cleanup!r}")
        raise
    else:
        await stack.aclose()


@dataclass
class Runtime:
    paths: ResolvedPaths
    descriptor: InstanceDescriptor
    configuration: Configuration
    database: Database
    execution: ExecutionManager
    time: TimeService
    observability: Observability
    state: str = "starting"

    def status(self) -> dict[str, object]:
        effective = self.configuration.snapshot.typed_values()
        configured_models = bool(effective.models)
        return {
            "state": self.state,
            "instance_id": self.descriptor.instance_id,
            "name": effective.instance.name,
            "config_version": self.configuration.snapshot.version,
            "configuration": self.configuration.status(),
            "capabilities": {
                "configuration": "ready",
                "database": "ready",
                "execution": "ready",
                "time": "ready",
                "models": "unavailable" if configured_models else "unconfigured",
                "network": "disabled",
                "content": "unavailable",
                "durable_execution": "unavailable",
            },
            "in_flight": self.execution.in_flight,
            "model_execution": "not_implemented",
            "paths": self.paths.to_dict(),
        }


async def initialize_instance(
    locator: InstanceLocator | Path | str | None = None,
    *,
    layout: str = "portable",
    name: str = "default",
    environment: Mapping[str, str] | None = None,
) -> InstanceDescriptor:
    selected = InstanceLocator.resolve(locator, environment)
    if layout not in {"portable", "platform"}:
        raise NousError(
            "configuration.invalid_value", "Instance layout must be portable or platform."
        )
    existing = selected.config_file.is_file()
    created_identity: str | None = None
    if not existing:
        if selected.directory.exists() and any(selected.directory.iterdir()):
            raise NousError(
                "instance.not_empty",
                "The instance directory contains files and has no valid nous.toml.",
                details={"path": str(selected.directory)},
            )
        selected.directory.mkdir(parents=True, exist_ok=True)
        document = tomlkit.document()
        created_identity = str(uuid4())
        document["instance"] = {"id": created_identity, "name": name, "layout": layout}
        try:
            with selected.config_file.open("x", encoding="utf-8", newline="\n") as stream:
                stream.write(tomlkit.dumps(document))
        except FileExistsError as error:
            raise NousError(
                "instance.in_use", "Another operation is initializing this instance.", cause=error
            ) from error
    configuration = Configuration(selected.config_file, environment=environment)
    settings = configuration.snapshot.typed_values()
    if not existing and (
        settings.instance.id != created_identity or settings.instance.layout != layout
    ):
        raise NousError(
            "configuration.readonly",
            "Instance identity and layout are determined by initialization.",
        )
    descriptor = InstanceDescriptor(
        settings.instance.id, settings.instance.name, settings.instance.layout
    )
    paths = ResolvedPaths.resolve(selected, descriptor, settings.paths)
    with InstanceLock(selected.directory):
        if existing:
            binding = InstanceBinding.read(selected)
            binding.check_locations(selected, descriptor, paths, settings.database.filename)
        else:
            prepare_resource_directories(paths)
            binding = InstanceBinding.create(
                selected, descriptor, paths, settings.database.filename
            )
        with InstanceLock(paths.data, data=True):
            if existing:
                binding.check_resources(paths)
            async with Database.open(
                paths.resolve_owned("data", settings.database.filename),
                settings.database,
                descriptor.instance_id,
                create=not existing,
                database_id=str(binding.database_id),
            ) as database:
                await bind_domains(database, settings.time.domains)
                await database.check()
                if not existing:
                    binding.write(selected, paths)
            paths.create_runtime_directories()
    return descriptor


@asynccontextmanager
async def open_instance(
    locator: InstanceLocator | Path | str | None = None,
    *,
    environment: Mapping[str, str] | None = None,
    overrides: Mapping[str, object] | None = None,
) -> AsyncGenerator[Runtime]:
    startup_id = str(uuid4())
    logs = LoggingSession(startup_id)
    runtime: Runtime | None = None
    async with _resources() as stack:
        leases = ExitStack()
        stack.callback(leases.close)
        stack.callback(logs.close)
        selected = InstanceLocator.resolve(locator, environment)
        logger.info("runtime.starting")
        configuration = Configuration(
            selected.config_file, environment=environment, overrides=overrides, running=True
        )
        settings = configuration.snapshot.typed_values()
        descriptor = InstanceDescriptor(
            settings.instance.id, settings.instance.name, settings.instance.layout
        )
        paths = ResolvedPaths.resolve(selected, descriptor, settings.paths)
        leases.enter_context(InstanceLock(selected.directory))
        binding = InstanceBinding.read(selected)
        binding.check_locations(selected, descriptor, paths, settings.database.filename)
        leases.enter_context(InstanceLock(paths.data, data=True))
        binding.check_resources(paths)
        paths.create_runtime_directories()
        logs.configure(settings.logging, paths.logs, descriptor.instance_id)
        observability = Observability(settings.tracing, settings.metrics, descriptor.instance_id)
        stack.callback(observability.close)
        storage = await stack.enter_async_context(AsyncExitStack())
        execution = ExecutionManager(settings.execution, lambda: configuration.snapshot)

        async def close_execution() -> None:
            with operation("runtime.shutdown"):
                await execution.aclose(
                    configuration.snapshot.typed_values().runtime.shutdown_grace_seconds
                )

        stack.push_async_callback(close_execution)
        try:
            with operation("runtime.startup"):
                database = await storage.enter_async_context(
                    Database.open(
                        paths.resolve_owned("data", settings.database.filename),
                        settings.database,
                        descriptor.instance_id,
                        database_id=str(binding.database_id),
                    )
                )
                await bind_domains(database, settings.time.domains)

                async def binding_check(
                    old: ConfigSnapshot, candidate: ConfigSnapshot, changed: Sequence[str]
                ) -> None:
                    before = old.typed_values()
                    after = candidate.typed_values()
                    changed_paths = set(changed)
                    fixed = {
                        "instance.id",
                        "instance.layout",
                        "paths.data_dir",
                        "paths.content_dir",
                        "paths.index_dir",
                        "database.filename",
                    }
                    if changed_paths & fixed:
                        raise NousError(
                            "configuration.relocation_required",
                            "Existing instance paths and identity require an explicit relocation operation.",
                            details={"fields": sorted(changed_paths & fixed)},
                        )
                    if before.time.domains != after.time.domains:
                        await check_domains(database, after.time.domains)

                async def binding_apply(candidate: ConfigSnapshot) -> None:
                    await bind_domains(database, candidate.typed_values().time.domains)

                def live_apply(snapshot: ConfigSnapshot) -> None:
                    logs.set_level(snapshot.typed_values().logging.level)

                configuration.binding_check = binding_check
                configuration.binding_apply = binding_apply
                configuration.live_apply = live_apply
                runtime = Runtime(
                    paths,
                    descriptor,
                    configuration,
                    database,
                    execution,
                    TimeService(database),
                    observability,
                    "ready",
                )
            logger.info("runtime.ready")
            yield runtime
        except BaseException:
            if runtime is not None:
                runtime.state = "failed"
            logger.info("runtime.failed")
            raise
        finally:
            if runtime is not None and runtime.state != "failed":
                runtime.state = "stopping"
    if runtime is not None:
        runtime.state = "stopped"
