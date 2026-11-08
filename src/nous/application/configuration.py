"""One configuration change operation for offline and running instances."""

from __future__ import annotations

import asyncio
from collections.abc import AsyncGenerator, Mapping
from contextlib import asynccontextmanager
from pathlib import Path
from typing import cast

from pydantic import JsonValue

from nous.cognition.scope import NamespaceOperations
from nous.common.errors import NousError
from nous.common.operations import add_error_context
from nous.configuration import ConfigSnapshot, Configuration
from nous.configuration.values import Object, get_path, normalize_patch, set_path
from nous.models.embedding_store import EmbeddingSpaceRegistry
from nous.models.embeddings import resolved_space
from nous.observability.assembly import LoggingSession
from nous.runtime.bindings import InstanceBinding
from nous.runtime.execution import ExecutionManager
from nous.runtime.instance import InstanceDescriptor, InstanceLocator, InstanceLock
from nous.runtime.paths import ResolvedPaths
from nous.storage.database import Database
from nous.temporal.domains import bind_domains, check_domains


class ConfigurationOperations:
    """Coordinate configuration changes for an open or offline instance.

    Online operations borrow the runtime's database and logging session; the
    offline context acquires verified resources as needed. Changes check durable
    bindings before saving. A saved change may require restart, and failed live
    adoption leaves the previous effective snapshot active.
    """

    def __init__(
        self,
        configuration: Configuration,
        locator: InstanceLocator,
        binding: InstanceBinding,
        paths: ResolvedPaths,
        *,
        execution: ExecutionManager,
        database: Database | None = None,
        logs: LoggingSession | None = None,
    ) -> None:
        self._configuration = configuration
        self._locator = locator
        self._binding = binding
        self._paths = paths
        self._database = database
        self._logs = logs
        self._execution = execution
        self._lock = asyncio.Lock()

    @property
    def snapshot(self) -> ConfigSnapshot:
        return self._configuration.snapshot

    @property
    def version(self) -> int:
        return self._configuration.version

    @property
    def pending_restart(self) -> bool:
        return self._configuration.pending_restart

    def status(self) -> Object:
        return self._configuration.status()

    def saved(self) -> Object:
        return self._configuration.saved()

    def explain(
        self, path: str, scope: str = "instance", *, namespace_file: Path | str | None = None
    ) -> Object:
        return self._configuration.explain(path, scope, namespace_file=namespace_file)

    def resolve(
        self,
        scope: str = "instance",
        overrides: Mapping[str, object] | None = None,
        *,
        namespace_file: Path | str | None = None,
        task_overrides: Mapping[str, object] | None = None,
    ) -> ConfigSnapshot:
        return self._configuration.resolve(
            scope,
            overrides,
            namespace_file=namespace_file,
            task_overrides=task_overrides,
        )

    @asynccontextmanager
    async def _binding_database(self) -> AsyncGenerator[Database]:
        """Borrow an open database or acquire the verified offline resource only when needed."""
        if self._database is not None:
            yield self._database
            return
        settings = self._configuration.snapshot.values
        descriptor = InstanceDescriptor(
            settings.instance.id, settings.instance.name, settings.instance.layout
        )
        try:
            with InstanceLock(self._paths.data, data=True):
                self._binding.check_resources(self._paths)
                async with Database.open(
                    self._paths.resolve_owned("data", settings.database.filename),
                    settings.database,
                    descriptor.instance_id,
                    database_id=str(self._binding.database_id),
                ) as database:
                    yield database
        except NousError:
            raise
        except Exception as error:
            raise NousError(
                "configuration.binding_check_unavailable",
                "The database could not be opened to check persistent bindings.",
                cause=error,
            ) from error

    async def _check_bindings(
        self, before: ConfigSnapshot, after: ConfigSnapshot, *, saved_namespace: str | None = None
    ) -> None:
        """Check persistent time and space identities before saving their changed definitions."""
        settings = after.values
        descriptor = InstanceDescriptor(
            settings.instance.id, settings.instance.name, settings.instance.layout
        )
        paths = ResolvedPaths.resolve(self._locator, descriptor, settings.paths)
        self._binding.check_locations(self._locator, descriptor, paths, settings.database.filename)
        old = before.values
        time_changed = old.time.domains != settings.time.domains
        previous_spaces = {key: resolved_space(before, key) for key in old.embedding.spaces}
        desired_spaces = {key: resolved_space(after, key) for key in settings.embedding.spaces}
        namespace_changed = old.cognition.default_namespace != settings.cognition.default_namespace
        if (
            time_changed
            or previous_spaces != desired_spaces
            or namespace_changed
            or saved_namespace is not None
        ):
            async with self._binding_database() as database:
                if namespace_changed or saved_namespace is not None:
                    namespaces = NamespaceOperations(
                        database, self._execution, self, descriptor.instance_id
                    )
                    async with database.read() as tx:
                        for selector in {settings.cognition.default_namespace, saved_namespace} - {
                            None
                        }:
                            if selector is not None:
                                await namespaces.get_in(tx, selector)
                if time_changed:
                    await check_domains(database, settings.time.domains)
                if previous_spaces != desired_spaces:
                    await EmbeddingSpaceRegistry(database).check_snapshot(after)

    async def _apply_time(self, candidate: ConfigSnapshot) -> None:
        if self._database is None:
            raise NousError(
                "configuration.binding_check_unavailable",
                "Time binding application is unavailable.",
            )
        await bind_domains(self._database, candidate.values.time.domains)

    def _apply_live(self, candidate: ConfigSnapshot) -> None:
        if self._logs is None:
            raise NousError(
                "configuration.live_apply_unavailable", "Live logging application is unavailable."
            )
        self._logs.set_level(candidate.values.logging.level)

    async def update(
        self,
        patch: Mapping[str, object],
        scope: str = "instance",
        expected_version: int | None = None,
        *,
        namespace_file: Path | str | None = None,
    ) -> Object:
        """Validate and save a configuration patch, then apply eligible changes.

        Args:
            patch: Configuration paths and values to change.
            scope: Instance or namespace scope.
            expected_version: If supplied, reject a stale configuration version.
            namespace_file: Namespace configuration file for namespace scope.

        Returns:
            Change details including the effective and restart state.

        Raises:
            NousError: Validation, binding, save, or application failed. For
                ``configuration.apply_failed``, the file was saved but the old
                effective snapshot remains active; inspect error details and
                correct the saved settings or dependency before restarting.
        """
        return await self._change(
            normalize_patch(patch), (), scope, expected_version, namespace_file
        )

    async def unset(
        self,
        path: str,
        scope: str = "instance",
        expected_version: int | None = None,
        *,
        namespace_file: Path | str | None = None,
    ) -> Object:
        return await self._change({}, (path,), scope, expected_version, namespace_file)

    async def remove_resource(self, path: str, expected_version: int | None = None) -> Object:
        resource = path.count(".") == 1 and path.split(".")[0] in {
            "providers",
            "models",
            "presets",
            "tasks",
            "credentials",
        }
        space = path.startswith("embedding.spaces.") and path.count(".") == 2
        if not (resource or space):
            raise NousError("configuration.resource_invalid", "Use a resource collection and ID")
        return await self._change({}, (path,), "instance", expected_version, None)

    async def reload(
        self,
        expected_version: int | None = None,
        *,
        scope: str = "instance",
        namespace_file: Path | str | None = None,
    ) -> Object:
        return await self._change({}, (), scope, expected_version, namespace_file, reload=True)

    async def _change(
        self,
        patch: Object,
        removals: tuple[str, ...],
        scope: str,
        expected_version: int | None,
        namespace_file: Path | str | None,
        reload: bool = False,
    ) -> Object:
        async with self._execution.operation("configuration.change"), self._lock:
            try:
                return await self._change_locked(
                    patch, removals, scope, expected_version, namespace_file, reload
                )
            except NousError as error:
                if "file_saved" not in error.details:
                    recovery = (
                        "Correct the saved file and call reload again; the effective configuration is unchanged."
                        if reload
                        else "Correct the reported input or binding problem and retry; this operation did not save the file."
                    )
                    error.details.update(
                        self._configuration.failure_details(
                            self._configuration.config_file
                            if scope == "instance"
                            else Path(namespace_file or ""),
                            phase="validation",
                            file_saved=reload and error.code != "configuration.file_not_found",
                            write_performed=False,
                            recovery=recovery,
                        )
                    )
                raise
            except OSError as error:
                target = (
                    self._configuration.config_file
                    if scope == "instance"
                    else Path(namespace_file or "")
                )
                raise NousError(
                    "configuration.read_failed",
                    "The configuration file could not be read; the effective configuration is unchanged.",
                    details=self._configuration.failure_details(
                        target,
                        phase="read",
                        file_saved=False,
                        write_performed=False,
                        recovery="Correct filesystem access and retry the operation.",
                    ),
                    cause=error,
                ) from error

    async def _change_locked(
        self,
        patch: Object,
        removals: tuple[str, ...],
        scope: str,
        expected_version: int | None,
        namespace_file: Path | str | None,
        reload: bool = False,
    ) -> Object:
        """Save desired state before applying bindings; publish only after successful adoption."""
        configuration = self._configuration
        selector = get_path(patch, "cognition.default_namespace")
        if isinstance(selector, str):
            async with self._binding_database() as database:
                namespaces = NamespaceOperations(
                    database, self._execution, self, configuration.snapshot.values.instance.id
                )
                resolved = await namespaces.resolve(selector)
            patch = normalize_patch(patch)
            set_path(patch, "cognition.default_namespace", resolved.id)
        change = configuration.prepare_change(
            patch,
            removals,
            scope,
            expected_version,
            namespace_file,
            reload=reload,
        )
        try:
            await self._check_bindings(
                configuration.snapshot,
                change.candidate,
                saved_namespace=cast(
                    str | None, get_path(change.saved, "cognition.default_namespace")
                ),
            )
        except Exception as error:
            details = configuration.failure_details(
                change.target,
                phase="binding_check",
                file_saved=reload,
                write_performed=False,
                recovery="Correct the reported binding problem and retry.",
            )
            if isinstance(error, NousError):
                error.details.update(details)
                raise
            raise NousError(
                "configuration.binding_check_unavailable",
                "Persistent bindings could not be checked.",
                details=details,
                cause=error,
            ) from error
        modes = configuration.change_modes(change)
        pending = configuration.pending_restart or "restart" in modes
        candidate = configuration.save_change(change)
        if configuration.running and not pending and scope == "instance":
            binding_application = "not_run"
            live_application = "not_run"
            phase = "binding_apply"
            try:
                if any(path.startswith("time.domains.") for path in change.changed):
                    await self._apply_time(candidate)
                    binding_application = "applied"
                phase = "live_apply"
                if "live" in modes:
                    self._apply_live(candidate)
                    live_application = "applied"
            except BaseException as error:
                configuration.mark_apply_failure({})
                failure = configuration.failure_details(
                    change.target,
                    phase=phase,
                    file_saved=True,
                    write_performed=not reload,
                    recovery="Correct the saved settings or failed dependency, then stop and reopen the instance.",
                )
                failure["paths"] = cast(JsonValue, list(change.paths))
                failure["binding_application"] = (
                    "unconfirmed" if phase == "binding_apply" else binding_application
                )
                failure["live_application"] = (
                    "failed" if phase == "live_apply" else live_application
                )
                failure["cause_code"] = (
                    error.code if isinstance(error, NousError) else type(error).__name__
                )
                if isinstance(error, NousError):
                    failure["cause_details"] = error.details
                configuration.mark_apply_failure(dict(failure))
                if not isinstance(error, Exception):
                    add_error_context(error, failure)
                    raise
                raise NousError(
                    "configuration.apply_failed",
                    "Configuration was saved but could not be applied; the previous effective snapshot remains active and restart is required.",
                    details=failure,
                    cause=error,
                ) from error
        configuration.publish_change(change, candidate, pending=pending)
        return configuration.change_result(change)


@asynccontextmanager
async def open_configuration_operations(
    locator: InstanceLocator | Path | str | None = None,
    *,
    environment: Mapping[str, str] | None = None,
) -> AsyncGenerator[ConfigurationOperations]:
    """Borrow offline configuration operations while holding the instance lock.

    The returned service is valid only within the context. Database resources
    needed for binding checks are opened and closed by individual changes.
    """
    selected = InstanceLocator.resolve(locator, environment)
    with InstanceLock(selected.directory):
        configuration = Configuration(selected.config_file, environment=environment)
        settings = configuration.snapshot.values
        descriptor = InstanceDescriptor(
            settings.instance.id, settings.instance.name, settings.instance.layout
        )
        paths = ResolvedPaths.resolve(selected, descriptor, settings.paths)
        binding = InstanceBinding.read(selected)
        binding.check_locations(selected, descriptor, paths, settings.database.filename)
        execution = ExecutionManager(settings.execution, lambda: configuration.snapshot)
        try:
            yield ConfigurationOperations(
                configuration, selected, binding, paths, execution=execution
            )
        finally:
            await execution.aclose(settings.runtime.shutdown_grace_seconds)
