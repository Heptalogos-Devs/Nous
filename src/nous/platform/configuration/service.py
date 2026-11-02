"""Whole-key overrides, scoped inheritance and immutable change history."""

import os
import tempfile
import tomllib
from collections.abc import AsyncGenerator, Callable, Mapping
from contextlib import asynccontextmanager
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from types import MappingProxyType
from typing import Literal

import anyio
import tomlkit
from filelock import AsyncFileLock
from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.configuration import ConfigurationCatalog, ConfigurationDefinition
from pydantic import BaseModel, JsonValue, TypeAdapter

from nous.platform.errors import NousError


@dataclass(frozen=True)
class ConfigurationEntry:
    default: BaseModel
    instance_management: bool = False
    scopes: tuple[Literal["instance", "namespace"], ...] = ("instance", "namespace")
    validator: Callable[[BaseModel], None] | None = None
    schema: Callable[[], dict[str, JsonValue]] | None = None

    def parse(self, value: object) -> BaseModel:
        parsed = type(self.default).model_validate(value)
        if self.validator is not None:
            self.validator(parsed)
        return parsed


@dataclass(frozen=True)
class ConfigurationSnapshot:
    scopes: Mapping[str, Mapping[str, object]]
    applied_at: Mapping[str, str]
    versions: Mapping[str, int]


@dataclass(frozen=True)
class PreparedConfiguration:
    scope: str
    values: Mapping[str, object]
    version: int
    changed: tuple[str, ...] = ()


class ConfigurationValues:
    def __init__(self, entries: dict[str, ConfigurationEntry]) -> None:
        self._entries = dict(entries)
        self._defaults = MappingProxyType(
            {
                key: self._freeze(entry.parse(entry.default).model_dump(mode="json"))
                for key, entry in self._entries.items()
            }
        )
        self._default_applied_at = self._now()
        self._snapshot = ConfigurationSnapshot(
            scopes=MappingProxyType({}),
            applied_at=MappingProxyType({}),
            versions=MappingProxyType({}),
        )

    @staticmethod
    def _now() -> str:
        return datetime.now(UTC).isoformat()

    @staticmethod
    def _freeze(value: object) -> object:
        if isinstance(value, Mapping):
            return MappingProxyType(
                {str(key): ConfigurationValues._freeze(item) for key, item in value.items()}
            )
        if isinstance(value, (list, tuple)):
            return tuple(ConfigurationValues._freeze(item) for item in value)
        return value

    @staticmethod
    def mutable(value: object) -> object:
        if isinstance(value, Mapping):
            return {str(key): ConfigurationValues.mutable(item) for key, item in value.items()}
        if isinstance(value, tuple):
            return [ConfigurationValues.mutable(item) for item in value]
        return value

    def default(self, key: str) -> dict[str, JsonValue]:
        self.entry(key)
        return TypeAdapter(dict[str, JsonValue]).validate_python(
            self.mutable(self._defaults[key])
        )

    def scoped(self, scope: str) -> dict[str, JsonValue]:
        values = self._snapshot.scopes.get(scope, {})
        return TypeAdapter(dict[str, JsonValue]).validate_python(self.mutable(values))

    def publish(self, scope: str, values: dict[str, JsonValue], *, version: int) -> str:
        scopes = dict(self._snapshot.scopes)
        scopes[scope] = self._freeze(values)
        applied_at = dict(self._snapshot.applied_at)
        applied_at[scope] = self._now()
        versions = dict(self._snapshot.versions)
        versions[scope] = version
        self._snapshot = ConfigurationSnapshot(
            scopes=MappingProxyType(scopes),
            applied_at=MappingProxyType(applied_at),
            versions=MappingProxyType(versions),
        )
        return applied_at[scope]

    def version(self, scope: str) -> int:
        return self._snapshot.versions.get(scope, 0)

    @property
    def snapshot(self) -> ConfigurationSnapshot:
        return self._snapshot

    @staticmethod
    def merge(base: dict[str, JsonValue], override: dict[str, JsonValue]) -> dict[str, JsonValue]:
        result = dict(base)
        for key, value in override.items():
            previous = result.get(key)
            result[key] = (
                ConfigurationValues.merge(previous, value)
                if isinstance(previous, dict) and isinstance(value, dict)
                else value
            )
        return result

    def entry(self, key: str) -> ConfigurationEntry:
        try:
            return self._entries[key]
        except KeyError as error:
            raise NousError(
                "configuration_key_not_found",
                "Configuration key is not registered",
                status=404,
                context={"key": key},
            ) from error

    def scopes(self, ctx: CallContext) -> tuple[str, ...]:
        return ("instance", f"namespace:{ctx.namespace_id}")

    def effective(
        self,
        ctx: CallContext | None,
        key: str,
        *,
        scopes: tuple[str, ...] | None = None,
    ) -> dict[str, object]:
        entry = self.entry(key)
        if scopes is None:
            if ctx is None:
                raise ValueError("Scoped configuration requires a context")
            scopes = self.scopes(ctx)
        value = self.default(key)
        origin = "default"
        applied_at = self._default_applied_at
        snapshot = self._snapshot
        for scope in scopes:
            if scope.split(":", 1)[0] not in entry.scopes:
                continue
            scoped_values = snapshot.scopes.get(scope)
            if scoped_values is None or key not in scoped_values:
                continue
            override = TypeAdapter(dict[str, JsonValue]).validate_python(
                self.mutable(scoped_values[key])
            )
            value = self.merge(value, override)
            origin = scope
            applied_at = snapshot.applied_at.get(scope, applied_at)
        resolved = entry.parse(value).model_dump(mode="json")
        return {
            "key": key,
            "value": resolved,
            "origin": origin,
            "effective_at": applied_at,
            "version": snapshot.versions.get(origin, 0),
        }

    def policy[P: BaseModel](self, ctx: CallContext, key: str, model: type[P]) -> P:
        value = self.effective(ctx, key)
        return model.model_validate(value["value"])


class ConfigurationService:
    def __init__(
        self, values: ConfigurationValues, *, instance: Path
    ) -> None:
        self.instance = instance.resolve()
        self.lock_path = self.instance / "run" / "configuration.lock"
        self.lock_path.parent.mkdir(parents=True, exist_ok=True)
        self.values = values
        self._entries = values._entries
        self._defaults = values._defaults

    @asynccontextmanager
    async def write_lock(self) -> AsyncGenerator[None]:
        async with AsyncFileLock(str(self.lock_path)):
            yield

    @staticmethod
    def toml_value(value: JsonValue) -> JsonValue:
        if isinstance(value, dict):
            return {
                key: ConfigurationService.toml_value(item)
                for key, item in value.items()
                if item is not None
            }
        if isinstance(value, list):
            return [ConfigurationService.toml_value(item) for item in value]
        return value

    def path(self, ctx: CallContext, scope: Literal["instance", "namespace"]) -> Path:
        return (
            self.instance / "config.toml"
            if scope == "instance"
            else self.instance / "namespaces" / ctx.namespace_id / "config.toml"
        )

    @staticmethod
    def file_version(path: Path) -> int:
        try:
            return path.stat().st_mtime_ns
        except FileNotFoundError:
            return 0

    def parse_document(self, content: str) -> dict[str, JsonValue]:
        raw = TypeAdapter(dict[str, JsonValue]).validate_python(tomllib.loads(content))
        result: dict[str, JsonValue] = {}

        def visit(value: dict[str, JsonValue], prefix: str = "") -> None:
            for name, item in value.items():
                key = prefix + name
                if key in self._entries:
                    result[key] = item
                elif isinstance(item, dict) and any(
                    candidate.startswith(key + ".") for candidate in self._entries
                ):
                    visit(item, key + ".")
                else:
                    raise DomainError(
                        "configuration_key_not_found",
                        "Configuration key is not registered",
                        key=key,
                    )

        visit(raw)
        return result

    def read_file(self, path: Path) -> dict[str, JsonValue]:
        try:
            return self.parse_document(path.read_text(encoding="utf-8"))
        except FileNotFoundError as error:
            raise DomainError(
                "configuration_file_not_found", "Configuration file does not exist", path=str(path)
            ) from error
        except (ValueError, TypeError) as error:
            raise DomainError("configuration_invalid", str(error), path=str(path)) from error

    @staticmethod
    def replace_file(path: Path, content: str) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        descriptor, name = tempfile.mkstemp(prefix=".config-", suffix=".toml", dir=path.parent)
        temporary = Path(name)
        try:
            with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as stream:
                stream.write(content)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, path)
        finally:
            temporary.unlink(missing_ok=True)

    def update_file(self, path: Path, values: dict[str, JsonValue | None]) -> str:
        document = (
            tomlkit.parse(path.read_text(encoding="utf-8")) if path.exists() else tomlkit.document()
        )
        for key, value in values.items():
            if value is None:
                if key in document:
                    del document[key]
            else:
                document[key] = self.toml_value(value)
        return tomlkit.dumps(document)

    def document_text(self, values: dict[str, JsonValue]) -> str:
        document = tomlkit.document()
        for key, value in values.items():
            document[key] = self.toml_value(value)
        return tomlkit.dumps(document)

    async def initialize(self) -> None:
        ctx = CallContext(
            caller_id="configuration", namespace_id="instance", credential_source_ids=None
        )
        path = self.path(ctx, "instance")
        async with self.write_lock():
            if not path.exists():
                document = tomlkit.document()
                for key, entry in self._entries.items():
                    if "instance" in entry.scopes:
                        document[key] = self.toml_value(self.values.default(key))
                await anyio.to_thread.run_sync(self.replace_file, path, tomlkit.dumps(document))
        await self.reload(ctx, "instance")
        directory = self.instance / "namespaces"
        if directory.is_dir():
            for file in sorted(directory.glob("*/config.toml")):
                scoped = ctx.model_copy(update={"namespace_id": file.parent.name})
                await self.reload(scoped, "namespace")

    async def load(
        self, ctx: CallContext, scope: Literal["instance", "namespace"] = "instance"
    ) -> dict[str, JsonValue]:
        path = self.path(ctx, scope)
        if scope == "namespace" and not path.exists():
            return {}
        return await anyio.to_thread.run_sync(self.read_file, path)

    def validate(
        self,
        ctx: CallContext,
        values: dict[str, JsonValue],
        scope: Literal["instance", "namespace"] = "instance",
    ) -> dict[str, JsonValue]:
        checked: dict[str, JsonValue] = {}
        inherited_scopes = () if scope == "instance" else ("instance",)
        for key, value in values.items():
            entry = self.entry(key)
            if scope not in entry.scopes:
                raise DomainError(
                    "configuration_scope_invalid",
                    "Configuration does not support this scope",
                    key=key,
                    scope=scope,
                )
            base = self.values.effective(ctx, key, scopes=inherited_scopes)["value"]
            override = TypeAdapter(dict[str, JsonValue]).validate_python(value)
            merged = ConfigurationValues.merge(
                TypeAdapter(dict[str, JsonValue]).validate_python(base), override
            )
            entry.parse(merged)
            checked[key] = override
        return checked

    def validate_document(
        self, ctx: CallContext, content: str, scope: Literal["instance", "namespace"]
    ) -> dict[str, JsonValue]:
        return self.validate(ctx, self.parse_document(content), scope)

    async def prepare_document_locked(
        self, ctx: CallContext, content: str, scope: Literal["instance", "namespace"]
    ) -> PreparedConfiguration:
        target = "instance" if scope == "instance" else f"namespace:{ctx.namespace_id}"
        values = self.validate_document(ctx, content, scope)
        previous = await self.load(ctx, scope)
        path = self.path(ctx, scope)
        current_content = (
            await anyio.to_thread.run_sync(path.read_text, "utf-8") if path.exists() else ""
        )
        changed = tuple(
            sorted(key for key in set(previous) | set(values) if previous.get(key) != values.get(key))
        )
        if current_content != content:
            await anyio.to_thread.run_sync(self.replace_file, path, content)
        return PreparedConfiguration(
            target,
            ConfigurationValues._freeze(values),
            self.file_version(path),
            changed,
        )
    def publish_prepared(self, prepared: PreparedConfiguration) -> str:
        values = TypeAdapter(dict[str, JsonValue]).validate_python(
            ConfigurationValues.mutable(prepared.values)
        )
        return self.values.publish(prepared.scope, values, version=prepared.version)

    async def reload(
        self,
        ctx: CallContext,
        scope: Literal["instance", "namespace"] = "instance",
    ) -> dict[str, object]:
        async with self.write_lock():
            return await self.reload_locked(ctx, scope)

    async def reload_locked(
        self,
        ctx: CallContext,
        scope: Literal["instance", "namespace"] = "instance",
    ) -> dict[str, object]:
        target = "instance" if scope == "instance" else "namespace:" + ctx.namespace_id
        raw = await self.load(ctx, scope)
        checked = self.validate(ctx, raw, scope)
        previous = self.values.scoped(target)
        changed = sorted(
            key for key in set(previous) | set(checked) if previous.get(key) != checked.get(key)
        )
        effective_at = self.values.publish(
            target, checked, version=self.file_version(self.path(ctx, scope))
        )
        return {
            "scope": target,
            "path": str(self.path(ctx, scope)),
            "changed": changed,
            "effective_at": effective_at,
            "version": self.values.version(target),
        }

    async def replace(
        self,
        ctx: CallContext,
        content: str,
        scope: Literal["instance", "namespace"] = "instance",
    ) -> dict[str, object]:
        async with self.write_lock():
            prepared = await self.prepare_document_locked(ctx, content, scope)
            effective_at = self.publish_prepared(prepared)
            return {
                "scope": prepared.scope,
                "path": str(self.path(ctx, scope)),
                "changed": list(prepared.changed),
                "effective_at": effective_at,
                "version": prepared.version,
            }

    async def export(
        self, ctx: CallContext, scope: Literal["instance", "namespace"] = "instance"
    ) -> str:
        path = self.path(ctx, scope)
        if not path.exists() and scope == "namespace":
            return ""
        return await anyio.to_thread.run_sync(path.read_text, "utf-8")

    def entry(self, key: str) -> ConfigurationEntry:
        return self.values.entry(key)

    def describe(self, key: str) -> ConfigurationDefinition:
        entry = self.entry(key)
        return ConfigurationDefinition(
            key=key,
            value_schema=entry.schema()
            if entry.schema
            else type(entry.default).model_json_schema(),
            default_value=self.values.default(key),
            write_access="instance_manage" if entry.instance_management else "manage",
            scopes=entry.scopes,
        )

    def catalog(self) -> ConfigurationCatalog:
        return ConfigurationCatalog(
            items=tuple(self.describe(key) for key in sorted(self._entries))
        )

    def scopes(self, ctx: CallContext) -> tuple[str, ...]:
        return self.values.scopes(ctx)

    async def effective(
        self,
        ctx: CallContext,
        key: str,
        *,
        scopes: tuple[str, ...] | None = None,
    ) -> dict[str, object]:
        return self.values.effective(ctx, key, scopes=scopes)

    async def effective_instance(self, key: str) -> dict[str, object]:
        entry = self.entry(key)
        if entry.scopes != ("instance",):
            raise ValueError("Instance resource resolution requires an instance-only definition")
        return self.values.effective(None, key, scopes=("instance",))

    async def set(
        self,
        ctx: CallContext,
        key: str,
        value: object,
        scope: Literal["instance", "namespace"] = "instance",
        *,
        expected_version: int | None = None,
    ) -> dict[str, object]:
        result = await self.update(ctx, {key: value}, scope, expected_version=expected_version)
        result["key"] = key
        result["value"] = self.values.scoped(
            "instance" if scope == "instance" else f"namespace:{ctx.namespace_id}"
        ).get(key)
        return result

    async def update(
        self,
        ctx: CallContext,
        values: dict[str, object | None],
        scope: Literal["instance", "namespace"] = "namespace",
        *,
        expected_version: int | None = None,
    ) -> dict[str, object]:
        async with self.write_lock():
            prepared = await self.prepare_update_locked(
                ctx, values, scope, expected_version=expected_version
            )
            effective_at = self.publish_prepared(prepared)
            return {
                "scope": prepared.scope,
                "values": {
                    key: self.values.effective(ctx, key)["value"] for key in values
                },
                "effective_at": effective_at,
                "changed": list(prepared.changed),
                "path": str(self.path(ctx, scope)),
                "version": prepared.version,
            }

    async def prepare_update_locked(
        self,
        ctx: CallContext,
        values: dict[str, object | None],
        scope: Literal["instance", "namespace"],
        *,
        expected_version: int | None = None,
    ) -> PreparedConfiguration:
        if not values:
            raise ValueError("Configuration update requires at least one value")
        path = self.path(ctx, scope)
        actual_version = self.file_version(path)
        if expected_version is not None and expected_version != actual_version:
            raise DomainError(
                "configuration_conflict",
                "Configuration changed since it was read",
                expected_version=expected_version,
                current_version=actual_version,
            )
        changes: dict[str, JsonValue | None] = {}
        for key, value in values.items():
            entry = self.entry(key)
            if scope not in entry.scopes:
                raise DomainError(
                    "configuration_scope_invalid",
                    "Configuration does not support this scope",
                    key=key,
                    scope=scope,
                )
            changes[key] = (
                None
                if value is None
                else TypeAdapter(dict[str, JsonValue]).validate_python(value)
            )
        previous = await self.load(ctx, scope)
        content = await anyio.to_thread.run_sync(self.update_file, path, changes)
        checked = self.validate(ctx, self.parse_document(content), scope)
        changed = tuple(
            sorted(key for key in set(previous) | set(checked) if previous.get(key) != checked.get(key))
        )
        if changed:
            await anyio.to_thread.run_sync(self.replace_file, path, content)
        return PreparedConfiguration(
            "instance" if scope == "instance" else f"namespace:{ctx.namespace_id}",
            ConfigurationValues._freeze(checked),
            self.file_version(path),
            changed,
        )

    async def policy[P: BaseModel](self, ctx: CallContext, key: str, model: type[P]) -> P:
        return self.values.policy(ctx, key, model)
