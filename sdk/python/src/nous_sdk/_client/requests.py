"""Normalize public operation envelopes before validating their owner contracts."""

from collections.abc import Mapping
from typing import Protocol
from uuid import uuid4

from pydantic import JsonValue

from ..operations import object_value
from ..types import JsonObject, Subject


class ScopedClient(Protocol):
    namespace_id: str | None
    subject_id: str | None

    async def subject(self, subject_id: str) -> Subject: ...


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


async def request_scope(client: ScopedClient, values: JsonObject) -> tuple[str, str]:
    subject = values.pop("subject_id", None) or client.subject_id
    namespace = values.pop("namespace_id", None) or client.namespace_id
    if not isinstance(subject, str):
        raise ValueError("operation requires subject_id")
    if not isinstance(namespace, str):
        namespace = (await client.subject(subject)).namespace_id
    return namespace, subject
