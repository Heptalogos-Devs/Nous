"""Registered configuration definitions shared by management clients."""

from typing import Literal

from pydantic import Field, JsonValue

from .common import Contract


class ConfigurationDefinition(Contract):
    key: str
    value_schema: dict[str, JsonValue]
    default_value: dict[str, JsonValue]
    write_access: Literal["manage", "instance_manage"]
    scopes: tuple[Literal["instance", "namespace", "subject"], ...] = Field(min_length=1)


class ConfigurationCatalog(Contract):
    items: tuple[ConfigurationDefinition, ...]
