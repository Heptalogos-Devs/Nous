"""Layered resolution, immutable operation snapshots and atomic TOML updates."""

import asyncio
import inspect
import json
import os
import tempfile
import types
from collections.abc import Awaitable, Callable, Iterator, Mapping, MutableMapping
from dataclasses import dataclass
from fractions import Fraction
from pathlib import Path
from types import MappingProxyType
from typing import Annotated, Union, cast, get_args, get_origin

import tomlkit
from pydantic import BaseModel, JsonValue, TypeAdapter, ValidationError
from pydantic_settings import BaseSettings, EnvSettingsSource, SettingsConfigDict
from pydantic_settings.sources.utils import parse_env_vars
from typing_extensions import TypeForm

from nous.runtime.errors import NousError

from .definitions import NousConfig

type Object = dict[str, object]
type Layer = tuple[str, str, Object]
type BindingCheck = Callable[
    [ConfigSnapshot, ConfigSnapshot, tuple[str, ...]], Awaitable[None] | None
]
type SnapshotCallback = Callable[[ConfigSnapshot], Awaitable[None] | None]


def object_map(value: object) -> Object:
    return cast(Object, value)


class EnvironmentSettings(NousConfig, BaseSettings):
    model_config = SettingsConfigDict(
        env_prefix="NOUS__", env_nested_delimiter="__", extra="forbid"
    )


class FrozenValues(Mapping[str, object]):
    """Read-only mappings support both settings.field and resource[key]."""

    __slots__ = ("_data",)
    _data: Mapping[str, object]

    def __init__(self, value: Mapping[str, object]) -> None:
        object.__setattr__(
            self, "_data", MappingProxyType({key: freeze(item) for key, item in value.items()})
        )

    def __setattr__(self, name: str, value: object) -> None:
        raise TypeError("Configuration snapshots are immutable")

    def __getitem__(self, key: str) -> object:
        return self._data[key]

    def __iter__(self) -> Iterator[str]:
        return iter(self._data)

    def __len__(self) -> int:
        return len(self._data)

    def __getattr__(self, name: str) -> object:
        try:
            return self._data[name]
        except KeyError as error:
            raise AttributeError(name) from error


def freeze(value: object) -> object:
    if isinstance(value, Mapping):
        return FrozenValues(cast(Mapping[str, object], value))
    if isinstance(value, (tuple, list)):
        return tuple(freeze(item) for item in cast(tuple[object, ...] | list[object], value))
    return value


def mutable(value: object) -> JsonValue:
    if isinstance(value, Mapping):
        return {key: mutable(item) for key, item in cast(Mapping[str, object], value).items()}
    if isinstance(value, (list, tuple)):
        return [mutable(item) for item in cast(tuple[object, ...] | list[object], value)]
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    raise TypeError("Unsupported configuration value")


@dataclass(frozen=True)
class ConfigSnapshot:
    values: FrozenValues
    version: int
    sources: FrozenValues
    state: str = "effective"

    def to_dict(self) -> Object:
        return object_map(mutable(self.values))

    def typed_values(self) -> NousConfig:
        """Return a typed independent copy; mutations cannot change this snapshot."""
        return NousConfig.model_validate(self.to_dict())


def merge(base: Mapping[str, object], override: Mapping[str, object]) -> Object:
    result = object_map(mutable(base))
    for key, value in override.items():
        result[key] = (
            merge(cast(Mapping[str, object], result[key]), cast(Mapping[str, object], value))
            if isinstance(result.get(key), Mapping) and isinstance(value, Mapping)
            else mutable(value)
        )
    return result


def leaves(value: Mapping[str, object], prefix: str = "") -> Iterator[tuple[str, object]]:
    for key, item in value.items():
        path = f"{prefix}.{key}" if prefix else key
        if isinstance(item, Mapping) and item:
            yield from leaves(cast(Mapping[str, object], item), path)
        else:
            yield path, item


def get_path(value: object, path: str, default: object = None) -> object:
    current = value
    for part in path.split("."):
        if not isinstance(current, Mapping) or part not in current:
            return default
        current = cast(MutableMapping[str, object], current[part])
    return current


def set_path(value: MutableMapping[str, object], path: str, item: object) -> None:
    parts = path.split(".")
    if not all(parts):
        raise NousError(
            "configuration.path_invalid", "Configuration path contains an empty segment"
        )
    current = value
    for part in parts[:-1]:
        if part not in current:
            current[part] = {}
        if not isinstance(current[part], Mapping):
            raise NousError("configuration.path_invalid", f"Cannot descend through {part}")
        current = cast(MutableMapping[str, object], current[part])
    current[parts[-1]] = item


def delete_path(value: MutableMapping[str, object], path: str) -> None:
    parts = path.split(".")
    current = value
    parents: list[tuple[MutableMapping[str, object], str]] = []
    for part in parts[:-1]:
        if part not in current or not isinstance(current[part], Mapping):
            return
        parents.append((current, part))
        current = cast(MutableMapping[str, object], current[part])
    current.pop(parts[-1], None)
    for parent, key in reversed(parents):
        if not parent[key]:
            parent.pop(key)


def normalize_patch(patch: Mapping[str, object]) -> Object:
    result: Object = {}
    for key, value in patch.items():
        if "." in key:
            set_path(result, key, value)
        else:
            result[key] = mutable(value)
    return result


class Configuration:
    def __init__(
        self,
        config_file: Path | str,
        *,
        environment: Mapping[str, str] | None = None,
        overrides: Mapping[str, object] | None = None,
        running: bool = False,
        binding_check: BindingCheck | None = None,
        binding_apply: SnapshotCallback | None = None,
        live_apply: SnapshotCallback | None = None,
    ) -> None:
        self.config_file = Path(config_file)
        self.environment = dict(os.environ if environment is None else environment)
        self.overrides = normalize_patch(overrides or {})
        self.running = running
        self.binding_check = binding_check
        self.binding_apply = binding_apply
        self.live_apply = live_apply
        self.pending_restart = False
        self._activation_failure: Object | None = None
        self._version = 1
        self._lock = asyncio.Lock()
        self._namespace_saved: dict[Path, Object] = {}
        self._namespace_active: dict[Path, Object] = {}
        self._schema: Object = NousConfig.model_json_schema()
        self._saved = self._read(self.config_file)
        # Materialize package defaults before source merging, including keyed resources.
        # A supplied domains map must retain the built-in utc definition.
        self._defaults = self._validate({"instance": self._saved.get("instance")})
        self._environment = self._environment_values()
        self._snapshot = self._resolve(self._saved)

    @property
    def snapshot(self) -> ConfigSnapshot:
        return self._snapshot

    @property
    def version(self) -> int:
        """Last accepted saved generation, including changes waiting for restart."""
        return self._version

    def status(self) -> Object:
        return {
            "file": str(self.config_file),
            "version": self._version,
            "effective_version": self._snapshot.version,
            "state": "saved_pending_restart" if self.pending_restart else self._snapshot.state,
            "pending_restart": self.pending_restart,
            "activation_failure": mutable(self._activation_failure),
        }

    def _failure_details(
        self, target: Path, *, phase: str, file_saved: bool, write_performed: bool, recovery: str
    ) -> dict[str, JsonValue]:
        return {
            "file": str(target),
            "file_saved": file_saved,
            "write_performed": write_performed,
            "state": "saved_pending_restart" if self.pending_restart else "rejected",
            "phase": phase,
            "version": self._version,
            "effective_version": self._snapshot.version,
            "pending_restart": self.pending_restart,
            "recovery": recovery,
        }

    @staticmethod
    def schema() -> Object:
        return NousConfig.model_json_schema()

    @staticmethod
    def _read(path: Path) -> Object:
        try:
            content = path.read_text(encoding="utf-8")
        except FileNotFoundError as error:
            raise NousError(
                "configuration.file_not_found", f"Configuration file does not exist: {path}"
            ) from error
        try:
            doc = tomlkit.parse(content)
            result = object_map(doc.unwrap())
        except Exception as error:
            raise NousError(
                "configuration.syntax_invalid", str(error), details={"file": str(path)}
            ) from error
        raw_metadata = result.pop("config", {})
        if not isinstance(raw_metadata, Mapping):
            raise NousError("configuration.invalid", "config must be a TOML table")
        metadata = cast(Object, raw_metadata)
        if set(metadata) - {"null_fields"}:
            raise NousError("configuration.field_unknown", "Only config.null_fields is reserved")
        nulls = metadata.get("null_fields", [])
        if not isinstance(nulls, list) or any(
            not isinstance(path, str) for path in cast(list[object], nulls)
        ):
            raise NousError(
                "configuration.null_invalid", "config.null_fields must be an array of field paths"
            )
        sentinel = object()
        for field in cast(list[str], nulls):
            if get_path(result, field, sentinel) is not sentinel:
                raise NousError(
                    "configuration.null_conflict",
                    f"{field} has a value and an explicit null in the same layer",
                )
            set_path(result, field, None)
        return result

    def _deref(self, node: Object) -> Object:
        if "$ref" in node:
            return {
                **object_map(object_map(self._schema["$defs"])[str(node["$ref"]).split("/")[-1]]),
                **{k: v for k, v in node.items() if k != "$ref"},
            }
        return node

    def _field(self, path: str) -> tuple[Object, Object]:
        node = self._schema
        metadata: Object = {
            "exposure": "advanced",
            "apply_mode": "next_operation",
            "impact": "ordinary",
            "scopes": ["instance"],
            "readonly": False,
            "secret_value": False,
        }
        for part in path.split("."):
            node = self._deref(node)
            variants = cast(list[Object], node.get("oneOf", node.get("anyOf", [])))
            for variant in variants:
                resolved = self._deref(variant)
                if part in object_map(resolved.get("properties", {})):
                    node = resolved
                    break
            if "properties" in node and part in object_map(node["properties"]):
                node = object_map(object_map(node["properties"])[part])
            elif node.get("patternProperties"):
                node = object_map(next(iter(object_map(node["patternProperties"]).values())))
            elif isinstance(node.get("additionalProperties"), dict):
                node = object_map(node["additionalProperties"])
            else:
                raise NousError(
                    "configuration.field_unknown",
                    f"Unknown configuration field: {path}",
                    details={"path": path},
                )
            metadata.update(object_map(node.get("x-nous", {})))
        return self._deref(node), metadata

    def _validate_layer(self, values: Mapping[str, object]) -> None:
        """Check present fields even when a higher source masks their values."""

        def check(value: object, annotation: object, path: str = "") -> None:
            origin = get_origin(annotation)
            if origin is Annotated:
                base = get_args(annotation)[0]
                if isinstance(base, type) and issubclass(base, BaseModel):
                    return check(value, cast(object, base), path)
                if get_origin(base) in {Union, types.UnionType}:
                    return check(value, cast(object, base), path)
            if origin in {Union, types.UnionType} and isinstance(value, Mapping):
                errors: list[Exception] = []
                for member in get_args(annotation):
                    if member is type(None):
                        continue
                    try:
                        check(cast(Mapping[str, object], value), member, path)
                        return
                    except (NousError, ValidationError) as error:
                        errors.append(error)
                raise NousError(
                    "configuration.invalid", f"{path}: no valid object variant"
                ) from errors[-1]
            if (
                isinstance(annotation, type)
                and issubclass(annotation, BaseModel)
                and isinstance(value, Mapping)
            ):
                for name, item in cast(Mapping[str, object], value).items():
                    if name not in annotation.model_fields:
                        raise NousError(
                            "configuration.field_unknown",
                            f"Unknown configuration field: {path}.{name}".lstrip("."),
                        )
                    field = annotation.model_fields[name]
                    child = (
                        Annotated[field.annotation, *field.metadata]
                        if field.metadata
                        else field.annotation
                    )
                    check(item, child, f"{path}.{name}".lstrip("."))
                return
            if origin is dict and isinstance(value, Mapping):
                key_type, value_type = get_args(annotation)
                for name, item in cast(Mapping[str, object], value).items():
                    TypeAdapter(key_type).validate_python(name, strict=True)
                    check(item, value_type, f"{path}.{name}".lstrip("."))
                return
            try:
                TypeAdapter[object](cast(TypeForm[object], annotation)).validate_python(
                    value, strict=True
                )
            except ValidationError as error:
                raise NousError(
                    "configuration.invalid",
                    f"{path}: {error.errors(include_input=False)[0]['msg']}",
                    details={"path": path},
                ) from error

        try:
            check(values, NousConfig)
        except ValidationError as error:
            raise NousError("configuration.invalid", str(error)) from error

    def _environment_values(self) -> Object:
        source = EnvSettingsSource(
            EnvironmentSettings,
            env_prefix="NOUS__",
            env_nested_delimiter="__",
            env_parse_none_str="null",
        )
        source.env_vars = parse_env_vars(
            self.environment, case_sensitive=False, ignore_empty=False, parse_none_str="null"
        )
        try:
            values: Object = source()
        except Exception as error:
            raise NousError("configuration.environment_invalid", str(error)) from error
        # Settings source owns nested parsing; reject typos it would otherwise ignore.
        for name in self.environment:
            if name.upper().startswith("NOUS__"):
                path = name[6:].lower().replace("__", ".")
                self._field(path)
        for path, value in list(leaves(values)):
            node, _ = self._field(path)
            types = {
                node.get("type"),
                *(choice.get("type") for choice in cast(list[Object], node.get("anyOf", []))),
            }
            if isinstance(value, str) and types.intersection({"integer", "number", "boolean"}):
                try:
                    set_path(values, path, json.loads(value))
                except ValueError as error:
                    raise NousError(
                        "configuration.environment_invalid",
                        f"Invalid typed environment value for {path}",
                    ) from error
        return values

    def _validate(self, values: Mapping[str, object]) -> Object:
        try:
            return NousConfig.model_validate(values).model_dump(mode="json")
        except ValidationError as error:
            issues = [
                {"path": ".".join(map(str, item["loc"])), "message": item["msg"]}
                for item in error.errors(include_input=False)
            ]
            raise NousError(
                "configuration.invalid",
                "; ".join(f"{item['path']}: {item['message']}" for item in issues),
                details={"issues": issues},
            ) from error

    def _environment_location(self, path: str) -> str:
        matches = [
            name
            for name in self.environment
            if name.upper().startswith("NOUS__")
            and (
                path == name[6:].lower().replace("__", ".")
                or path.startswith(name[6:].lower().replace("__", ".") + ".")
            )
        ]
        return max(matches, key=len) if matches else "NOUS__"

    def _resolve(
        self, saved: Object, *, scope: str = "instance", layers: tuple[Layer, ...] = ()
    ) -> ConfigSnapshot:
        self._validate_layer(saved)
        self._validate_layer(self._environment)
        self._validate_layer(self.overrides)
        values = merge(merge(merge(self._defaults, saved), self._environment), self.overrides)
        instance = self._validate(values)
        sources = {
            path: {"layer": "default", "location": "package"} for path, _ in leaves(instance)
        }
        for layer, location, data in (
            ("instance", str(self.config_file), saved),
            ("environment", "NOUS__", self._environment),
            ("startup", "explicit", self.overrides),
            *layers,
        ):
            if layer not in {"instance", "environment", "startup"}:
                self._check_scope(data, layer)
                self._validate_layer(data)
                values = merge(values, data)
            for path, _ in leaves(data):
                source_location = (
                    self._environment_location(path) if layer == "environment" else location
                )
                # A whole object or array overrides all descendants, including defaults.
                for leaf in list(sources):
                    if leaf == path or leaf.startswith(path + "."):
                        sources[leaf] = {"layer": layer, "location": source_location}
                sources[path] = {"layer": layer, "location": source_location}
        resolved = self._validate(values)
        if layers:
            for name in ("max_attempts", "max_input_tokens", "max_output_tokens"):
                if cast(int, object_map(resolved["model_execution"])[name]) > cast(
                    int, object_map(instance["model_execution"])[name]
                ):
                    raise NousError(
                        "configuration.limit_exceeded",
                        f"model_execution.{name} exceeds the instance ceiling",
                    )
        state = "effective" if self.running else "prospective"
        return ConfigSnapshot(FrozenValues(resolved), self._version, FrozenValues(sources), state)

    def _check_scope(self, values: Object, scope: str) -> None:
        for path, _ in leaves(values):
            _, metadata = self._field(path)
            if scope not in cast(list[str], metadata["scopes"]):
                raise NousError(
                    "configuration.scope_invalid",
                    f"{path} is not allowed in {scope} scope",
                    details={"path": path, "scope": scope},
                )

    def resolve(
        self,
        scope: str = "instance",
        overrides: Mapping[str, object] | None = None,
        *,
        namespace_file: Path | str | None = None,
        task_overrides: Mapping[str, object] | None = None,
    ) -> ConfigSnapshot:
        layers: list[Layer] = []
        new_namespace: tuple[Path, Object] | None = None
        if namespace_file is not None:
            path = Path(namespace_file).resolve()
            if path not in self._namespace_saved:
                data = self._read(path) if path.exists() else {}
                new_namespace = (path, data)
            else:
                data = self._namespace_active[path] if self.running else self._namespace_saved[path]
            layers.append(("namespace", str(path), data))
        if task_overrides:
            layers.append(("task", "task", normalize_patch(task_overrides)))
        if overrides:
            layers.append(
                (scope if scope != "instance" else "call", "explicit", normalize_patch(overrides))
            )
        # Operations inherit the currently published generation, never pending files.
        if self.running:
            values = self._snapshot.to_dict()
            sources = object_map(mutable(self._snapshot.sources))
            for layer, location, data in layers:
                self._check_scope(data, layer)
                self._validate_layer(data)
                values = merge(values, data)
                for path, _ in leaves(data):
                    sources[path] = {"layer": layer, "location": location}
            checked = self._validate(values)
            for name in ("max_attempts", "max_input_tokens", "max_output_tokens"):
                if cast(int, object_map(checked["model_execution"])[name]) > getattr(
                    self._snapshot.typed_values().model_execution, name
                ):
                    raise NousError(
                        "configuration.limit_exceeded",
                        f"model_execution.{name} exceeds the instance ceiling",
                    )
            result = ConfigSnapshot(
                FrozenValues(checked), self._snapshot.version, FrozenValues(sources), "effective"
            )
        else:
            result = self._resolve(self._saved, scope=scope, layers=tuple(layers))
        if new_namespace is not None:
            path, data = new_namespace
            self._namespace_saved[path] = data
            self._namespace_active[path] = data
        return result

    def catalog(self, level: str = "standard") -> list[Object]:
        levels = ["standard", "advanced", "developer", "internal"]
        if level not in levels:
            raise NousError("configuration.level_invalid", f"Unknown presentation level: {level}")
        result: list[Object] = []

        def visit(
            node: Object, path: str = "", inherited: Object | None = None, required: bool = False
        ) -> None:
            node = self._deref(node)
            meta: Object = {
                "exposure": "advanced",
                "apply_mode": "next_operation",
                "impact": "ordinary",
                "scopes": ["instance"],
                "readonly": False,
                "secret_value": False,
                **(inherited or {}),
                **object_map(node.get("x-nous", {})),
            }
            if "properties" in node:
                for key, child in object_map(node["properties"]).items():
                    visit(
                        object_map(child),
                        f"{path}.{key}" if path else key,
                        meta,
                        key in cast(list[str], node.get("required", [])),
                    )
            elif node.get("patternProperties"):
                child = object_map(next(iter(object_map(node["patternProperties"]).values())))
                visit(child, path + ".<id>", meta)
                actual: object = (
                    get_path(self._snapshot.values, path, {}) if hasattr(self, "_snapshot") else {}
                )
                if isinstance(actual, Mapping):
                    for key in cast(Mapping[str, object], actual):
                        visit(child, path + "." + key, meta)
            elif (
                isinstance(node.get("additionalProperties"), dict) and node["additionalProperties"]
            ):
                visit(object_map(node["additionalProperties"]), path + ".<id>", meta)
            elif levels.index(str(meta["exposure"])) <= levels.index(level):
                entry: Object = {
                    "path": path,
                    "schema": node,
                    "required": required,
                    "nullable": any(
                        item.get("type") == "null"
                        for item in cast(list[Object], node.get("anyOf", []))
                    ),
                    "description": node.get("description", ""),
                    **meta,
                }
                if "<id>" not in path and hasattr(self, "_snapshot"):
                    entry["default"] = get_path(
                        self._validate({"instance": self._snapshot.to_dict()["instance"]}), path
                    )
                elif "default" in node:
                    entry["default"] = node["default"]
                result.append(entry)

        visit(self._schema)
        return result

    def explain(
        self, path: str, scope: str = "instance", *, namespace_file: Path | str | None = None
    ) -> Object:
        node, metadata = self._field(path)
        if scope == "namespace" and namespace_file is not None:
            target = Path(namespace_file).resolve()
            current = self.resolve("namespace", namespace_file=target)
            scoped = self._namespace_saved[target]
            desired = self._resolve(self._saved, layers=(("namespace", str(target), scoped),))
        else:
            current = self._snapshot
            scoped = self._saved
            desired = self._resolve(self._saved)
        source = object_map(mutable(get_path(desired.sources, path, desired.sources.get(path, {}))))
        saved = get_path(scoped, path)
        return {
            "path": path,
            "saved_value": mutable(saved),
            "effective_value": mutable(get_path(current.values, path)),
            "desired_value": mutable(get_path(desired.values, path)),
            "source": source,
            "shadowed": source.get("layer") in {"environment", "startup"},
            "state": "saved_pending_restart" if self.pending_restart else current.state,
            "version": current.version,
            "saved_version": self._version,
            "activation_failure": mutable(self._activation_failure),
            "definition": {**metadata, "schema": node},
        }

    def saved(self) -> Object:
        return object_map(mutable(self._saved))

    def validate(
        self, values: Mapping[str, object] | None = None, scope: str = "instance"
    ) -> Object:
        if values is None:
            candidate = self._resolve(self._read(self.config_file))
        else:
            data = normalize_patch(values)
            self._check_scope(data, scope)
            candidate = self._resolve(data) if scope == "instance" else self.resolve(scope, data)
        return {
            "valid": True,
            "state": candidate.state,
            "checks": {
                "shape": "passed",
                "references": "passed",
                "bindings": "not_run",
                "model_execution": "not_implemented",
                "embedding_binding": "unavailable"
                if candidate.typed_values().embedding.spaces
                else "unconfigured",
                "embedding_remote_deployment": "not_verified"
                if candidate.typed_values().embedding.spaces
                else "not_applicable",
            },
        }

    async def update(
        self,
        patch: Mapping[str, object],
        scope: str = "instance",
        expected_version: int | None = None,
        *,
        namespace_file: Path | str | None = None,
    ) -> Object:
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
        self._field(path)
        return await self._change({}, (path,), scope, expected_version, namespace_file)

    async def remove_resource(self, path: str, expected_version: int | None = None) -> Object:
        if path.count(".") != 1 or path.split(".")[0] not in {
            "providers",
            "models",
            "presets",
            "tasks",
            "credentials",
        }:
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

    async def _call(self, callback: Callable[..., Awaitable[None] | None], *args: object) -> None:
        value = callback(*args)
        if inspect.isawaitable(value):
            await value

    async def _change(
        self,
        patch: Object,
        removals: tuple[str, ...],
        scope: str,
        expected_version: int | None,
        namespace_file: Path | str | None,
        reload: bool = False,
    ) -> Object:
        async with self._lock:
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
                        self._failure_details(
                            self.config_file if scope == "instance" else Path(namespace_file or ""),
                            phase="validation",
                            file_saved=reload and error.code != "configuration.file_not_found",
                            write_performed=False,
                            recovery=recovery,
                        )
                    )
                raise
            except OSError as error:
                target = self.config_file if scope == "instance" else Path(namespace_file or "")
                raise NousError(
                    "configuration.read_failed",
                    "The configuration file could not be read; the effective configuration is unchanged.",
                    details=self._failure_details(
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
        if expected_version is not None and expected_version != self._version:
            raise NousError(
                "configuration.version_conflict",
                "Configuration version changed",
                details={"version": self._version},
            )
        self._check_scope(patch, scope)
        for path in removals:
            self._check_scope(
                {path: None} if "." not in path else normalize_patch({path: None}), scope
            )
        target = (
            self.config_file
            if scope == "instance"
            else await asyncio.to_thread(Path(namespace_file).resolve)
            if namespace_file
            else None
        )
        if target is None or scope not in {"instance", "namespace"}:
            raise NousError(
                "configuration.scope_invalid",
                "Only instance and explicit namespace files can be saved",
            )
        before = self._read(target) if target.exists() else {}
        previously_saved = (
            self._saved if scope == "instance" else self._namespace_saved.get(target, before)
        )
        saved = merge(before, patch)
        for path in removals:
            delete_path(saved, path)
        self._validate_layer(saved)
        if scope == "instance":
            proposed = merge(merge(merge(self._defaults, saved), self._environment), self.overrides)
            proposed_utc = object_map(get_path(proposed, "time.domains.utc"))
            # UTC has a built-in persistent coordinate identity, including before DB access.
            try:
                changed_utc = (
                    proposed_utc.get("kind") != "utc"
                    or Fraction(str(proposed_utc.get("tick_seconds"))) != Fraction(1, 1000000)
                    or proposed_utc.get("epoch") != "Unix epoch"
                )
            except ValueError, ZeroDivisionError:
                changed_utc = False  # The ordinary model validator reports malformed scale input.
            if changed_utc:
                raise NousError(
                    "configuration.binding_conflict",
                    "A registered time domain cannot change its coordinate definition; use a new domain ID",
                    details={"domain_id": "utc"},
                )
        candidate = (
            self._resolve(saved)
            if scope == "instance"
            else self._resolve(self._saved, layers=(("namespace", str(target), saved),))
        )
        old_desired = (
            self._resolve(self._saved)
            if scope == "instance"
            else self._resolve(self._saved, layers=(("namespace", str(target), previously_saved),))
        )
        old_flat = dict(leaves(old_desired.to_dict()))
        new_flat = dict(leaves(candidate.to_dict()))
        missing = object()
        changed = sorted(
            path
            for path in old_flat.keys() | new_flat.keys()
            if old_flat.get(path, missing) != new_flat.get(path, missing)
        )
        saved_flat = dict(leaves(previously_saved))
        new_saved_flat = dict(leaves(saved))
        saved_changed = sorted(
            path
            for path in saved_flat.keys() | new_saved_flat.keys()
            if saved_flat.get(path, missing) != new_saved_flat.get(path, missing)
        )
        paths = sorted(set(changed) | set(saved_changed))
        embedding_paths = [
            path
            for path in paths
            if path == "embedding.spaces" or path.startswith("embedding.spaces.")
        ]
        # This release can parse space definitions but has no persisted-space owner.
        # Every management entry point must reject adopting a changed space identity.
        prior_settings = old_desired.typed_values()
        next_settings = candidate.typed_values()
        space_models = {
            space.model
            for space in (
                *prior_settings.embedding.spaces.values(),
                *next_settings.embedding.spaces.values(),
            )
        }
        embedding_paths.extend(
            path
            for path in paths
            if any(
                path == f"models.{model}.{field}"
                for model in space_models
                for field in ("provider", "model_name", "deployment_revision")
            )
        )
        if embedding_paths:
            raise NousError(
                "configuration.binding_check_unavailable",
                "Embedding space binding management is not available in this version.",
                details={"paths": embedding_paths},
            )
        for path in paths:
            _, meta = self._field(path)
            if meta.get("readonly"):
                raise NousError(
                    "configuration.readonly", f"{path} is read-only", details={"path": path}
                )
        binding_paths = [
            path for path in paths if self._field(path)[1]["apply_mode"] == "new_binding"
        ]
        if binding_paths:
            if self.binding_check is None:
                raise NousError(
                    "configuration.binding_check_unavailable",
                    "Persistent bindings could not be checked",
                    details={"paths": binding_paths},
                )
            try:
                await self._call(self.binding_check, self._snapshot, candidate, tuple(paths))
            except Exception as error:
                details = self._failure_details(
                    target,
                    phase="binding_check",
                    file_saved=reload,
                    write_performed=False,
                    recovery="Correct the saved file and reload again."
                    if reload
                    else "Correct the binding conflict or unavailable dependency and retry; this operation did not save the file.",
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
        pending = self.pending_restart or any(
            self._field(path)[1]["apply_mode"] == "restart" for path in changed
        )
        if self.running and not pending:
            if (
                any(path.startswith("time.domains.") for path in changed)
                and self.binding_apply is None
            ):
                raise NousError(
                    "configuration.binding_check_unavailable",
                    "Time domain binding application is unavailable.",
                )
            if (
                any(self._field(path)[1]["apply_mode"] == "live" for path in changed)
                and self.live_apply is None
            ):
                raise NousError(
                    "configuration.live_apply_unavailable",
                    "Live logging configuration application is unavailable.",
                )
        if not reload:
            try:
                self._write(target, saved, patch, removals)
            except Exception as error:
                raise NousError(
                    "configuration.save_failed",
                    "The configuration file was not saved; the effective configuration is unchanged.",
                    details=self._failure_details(
                        target,
                        phase="save",
                        file_saved=False,
                        write_performed=False,
                        recovery="Correct filesystem access and retry the same update.",
                    ),
                    cause=error,
                ) from error
        if scope == "instance":
            self._saved = saved
            self._version += 1
            candidate = ConfigSnapshot(
                candidate.values,
                self._version,
                candidate.sources,
                "effective" if self.running else "prospective",
            )
            if self.running and not pending:
                binding_application = "not_run"
                live_application = "not_run"
                phase = "binding_apply"
                try:
                    if self.binding_apply is not None and any(
                        path.startswith("time.domains.") for path in changed
                    ):
                        await self._call(self.binding_apply, candidate)
                        binding_application = "applied"
                    phase = "live_apply"
                    if self.live_apply is not None and any(
                        self._field(path)[1]["apply_mode"] == "live" for path in changed
                    ):
                        await self._call(self.live_apply, candidate)
                        live_application = "applied"
                except BaseException as error:
                    self.pending_restart = True
                    recovery = "Correct the saved settings or failed dependency, then stop and reopen the instance; startup rechecks bindings and applies the complete saved configuration."
                    failure = self._failure_details(
                        target,
                        phase=phase,
                        file_saved=True,
                        write_performed=not reload,
                        recovery=recovery,
                    )
                    failure["paths"] = cast(JsonValue, paths)
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
                    self._activation_failure = dict(failure)
                    if not isinstance(error, Exception):
                        error.add_note(json.dumps(failure, ensure_ascii=False))
                        raise
                    raise NousError(
                        "configuration.apply_failed",
                        "Configuration was saved but could not be applied; the previous effective snapshot remains active and restart is required.",
                        details=failure,
                        cause=error,
                    ) from error
                self._snapshot = candidate
                self._activation_failure = None
            elif not self.running:
                self._snapshot = candidate
            self.pending_restart = bool(self.running and pending)
        else:
            if target not in self._namespace_active:
                self._namespace_active[target] = previously_saved
            self._namespace_saved[target] = saved
            self._version += 1
            if not self.running or not pending:
                self._namespace_active[target] = saved
            if not self.running or not pending:
                self._snapshot = ConfigSnapshot(
                    self._snapshot.values,
                    self._version,
                    self._snapshot.sources,
                    self._snapshot.state,
                )
        state = (
            "saved_pending_restart"
            if self.running and pending
            else "applied"
            if self.running
            else "saved"
        )
        return {
            "state": state,
            "file": str(target),
            "paths": paths,
            "version": self._version,
            "effective_version": self._snapshot.version,
            "file_saved": True,
            "write_performed": not reload,
            "recovery": "Restart the instance to apply the complete saved configuration."
            if self.pending_restart
            else None,
            "activation_failure": mutable(self._activation_failure),
            "changes": [self.explain(path, scope, namespace_file=target) for path in paths],
            "pending_restart": self.pending_restart,
        }

    @staticmethod
    def _write(target: Path, saved: Object, patch: Object, removals: tuple[str, ...]) -> None:
        doc: Object = (
            object_map(tomlkit.parse(target.read_text(encoding="utf-8")))
            if target.exists()
            else object_map(tomlkit.document())
        )
        nulls = set(cast(list[str], object_map(doc.get("config", {})).get("null_fields", [])))
        for path in removals:
            delete_path(doc, path)
            nulls = {item for item in nulls if item != path and not item.startswith(path + ".")}
        for path, value in leaves(patch):
            nulls = {item for item in nulls if item != path and not item.startswith(path + ".")}
            delete_path(doc, path)
            if value is None:
                nulls.add(path)
            else:
                set_path(doc, path, value)
        if nulls:
            if "config" not in doc:
                doc["config"] = tomlkit.table()
            object_map(doc["config"])["null_fields"] = sorted(nulls)
        elif "config" in doc:
            object_map(doc["config"]).pop("null_fields", None)
            if not doc["config"]:
                del doc["config"]
        target.parent.mkdir(parents=True, exist_ok=True)
        fd, temporary = tempfile.mkstemp(prefix=".nous-config-", suffix=".toml", dir=target.parent)
        try:
            with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as stream:
                stream.write(tomlkit.dumps(doc))
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, target)
        except BaseException as error:
            try:
                Path(temporary).unlink(missing_ok=True)
            except OSError as cleanup_error:
                error.add_note(
                    f"Could not remove temporary configuration file {temporary}: {cleanup_error}"
                )
            raise
