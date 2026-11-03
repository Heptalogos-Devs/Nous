"""Metadata attached to the authoritative Pydantic fields."""

from typing import Required, TypedDict, Unpack, overload

from pydantic import BaseModel, ConfigDict, Field
from pydantic.fields import FieldInfo


class ConfigModel(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, strict=True, allow_inf_nan=False)


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
