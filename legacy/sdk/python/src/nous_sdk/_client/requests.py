"""Normalize public operation envelopes before validating their owner contracts."""

from collections.abc import Mapping
from typing import Protocol
from uuid import uuid4

from pydantic import JsonValue

from ..operations import object_value
from ..types import JsonObject


class ScopedClient(Protocol):
    namespace_id: str | None
    



def operation_values(request: Mapping[str, JsonValue]) -> JsonObject:
    nested = request.get("request", request.get("input", {}))
    values = object_value(nested)
    return {
        **{
            key: value
            for key, value in request.items()
            if key not in {"action", "request", "input"}
        },
        **values,
    }


def operation_identity(values: JsonObject) -> str:
    identity = values.pop("operation_id", None) or str(uuid4())
    if not isinstance(identity, str):
        raise ValueError("operation_id must be text")
    return identity


def required_text(values: JsonObject, key: str) -> str:
    value = values.pop(key, None)
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{key} requires a nonempty identity")
    return value


def resolve_scope(client: ScopedClient, values: Mapping[str, JsonValue], *, default_namespace: str | None = None) -> str | None:
    namespace = values.get('namespace_id', client.namespace_id) or default_namespace
    if namespace is not None and (not isinstance(namespace,str) or not namespace.strip()):
        raise ValueError('namespace_id must be nonempty text')
    return namespace

async def request_scope(client: ScopedClient, values: JsonObject) -> str:
    namespace = resolve_scope(client, values)
    values.pop('namespace_id',None)
    if namespace is None: raise ValueError('Bind or supply a Namespace')
    return namespace
