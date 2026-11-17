"""Immutable configuration snapshots and operations on layered values."""

from collections.abc import Iterator, Mapping, MutableMapping
from dataclasses import dataclass
from types import MappingProxyType
from typing import cast

from pydantic import BaseModel

from nous.common.errors import NousError

from .definitions import NousConfig
from .metadata import mutable

type Object = dict[str, object]


def object_map(value: object) -> Object:
    return cast(Object, value)


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


@dataclass(frozen=True)
class ConfigSnapshot:
    """Frozen effective configuration values, provenance and accepted generation.

    ``values`` is a validated, deeply immutable typed view retained by operations.
    ``to_dict`` creates an independent JSON-compatible copy for explicit editing.
    """

    values: NousConfig
    version: int
    sources: FrozenValues
    state: str = "effective"

    def to_dict(self) -> Object:
        return self.values.model_dump(mode="json")


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
        if isinstance(current, BaseModel):
            if part not in type(current).model_fields:
                return default
            current = getattr(current, part)
            continue
        if not isinstance(current, Mapping) or part not in current:
            return default
        current = cast(Mapping[str, object], current)[part]
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
