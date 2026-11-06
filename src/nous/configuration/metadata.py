"""Metadata attached to the authoritative Pydantic fields."""

from collections.abc import Mapping
from types import MappingProxyType
from typing import Annotated, Required, TypedDict, Unpack, cast, overload

from pydantic import (
    AfterValidator,
    BaseModel,
    BeforeValidator,
    ConfigDict,
    Field,
    JsonValue,
    SerializerFunctionWrapHandler,
    WrapSerializer,
)
from pydantic.fields import FieldInfo


class ConfigModel(BaseModel):
    model_config = ConfigDict(
        extra="forbid", frozen=True, strict=True, allow_inf_nan=False, validate_default=True
    )


def _readonly_map[K, V](value: Mapping[K, V]) -> Mapping[K, V]:
    return MappingProxyType(dict(value))


def _serialize_map(
    value: Mapping[object, object], handler: SerializerFunctionWrapHandler
) -> object:
    return handler(dict(value))


def _sequence(value: object) -> object:
    return tuple(cast(list[object], value)) if isinstance(value, list) else value


type ConfigMap[K, V] = Annotated[
    Mapping[K, V], AfterValidator(_readonly_map), WrapSerializer(_serialize_map)
]
type ConfigSequence[T] = Annotated[tuple[T, ...], BeforeValidator(_sequence)]
type ConfigJson = (
    None | bool | int | float | str | ConfigSequence[ConfigJson] | ConfigMap[str, ConfigJson]
)


def mutable(value: object) -> JsonValue:
    """Copy read-only configuration containers into JSON-compatible values."""
    if isinstance(value, BaseModel):
        return cast(JsonValue, value.model_dump(mode="json"))
    if isinstance(value, Mapping):
        return {key: mutable(item) for key, item in cast(Mapping[str, object], value).items()}
    if isinstance(value, (list, tuple)):
        return [mutable(item) for item in cast(tuple[object, ...] | list[object], value)]
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    raise TypeError("Unsupported configuration value")


class SettingOptions(TypedDict, total=False):
    description: Required[str]
    level: str
    apply: str
    impact: str
    scopes: tuple[str, ...]
    readonly: bool
    ge: float | None
    gt: float | None
    le: float | None
    min_length: int | None
    pattern: str | None


@overload
def setting[T](default: T, **options: Unpack[SettingOptions]) -> T: ...


@overload
def setting(**options: Unpack[SettingOptions]) -> FieldInfo: ...


def setting(
    default: object = ...,
    *,
    description: str,
    level: str = "advanced",
    apply: str = "next_operation",
    impact: str = "ordinary",
    scopes: tuple[str, ...] = ("instance",),
    readonly: bool = False,
    ge: float | None = None,
    gt: float | None = None,
    le: float | None = None,
    min_length: int | None = None,
    pattern: str | None = None,
) -> object:
    return Field(
        default,
        description=description,
        json_schema_extra={
            "x-nous": {
                "exposure": level,
                "apply_mode": apply,
                "impact": impact,
                "scopes": list(scopes),
                "readonly": readonly,
                "secret_value": False,
            }
        },
        ge=ge,
        gt=gt,
        le=le,
        min_length=min_length,
        pattern=pattern,
    )
