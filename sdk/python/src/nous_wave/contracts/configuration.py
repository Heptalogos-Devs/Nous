"""Registered configuration definitions shared by management clients."""

from typing import Literal

from pydantic import JsonValue

from .common import Contract


class ConfigurationDefinition(Contract):
    key: str
    value_schema: dict[str, JsonValue]
    default_value: dict[str, JsonValue]
    write_access: Literal["manage", "instance_manage"]


class ConfigurationCatalog(Contract):
    items: tuple[ConfigurationDefinition, ...]
