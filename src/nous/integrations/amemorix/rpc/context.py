"""Authenticated protocol metadata becomes the same trusted Nous caller context."""

from __future__ import annotations

from typing import TYPE_CHECKING
from uuid import uuid4

from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.credentials import Capability
from nous_sdk.types import JsonObject
from pydantic import JsonValue, TypeAdapter

from nous.identity.credentials import Principal

if TYPE_CHECKING:
    from nous.application import Application


def object_field(values: JsonObject, name: str) -> JsonObject:
    return TypeAdapter(JsonObject).validate_python(values.get(name, {}))


def text_field(values: JsonObject, name: str, default: str = "") -> str:
    value = values.get(name, default)
    if not isinstance(value, str):
        raise DomainError("invalid_argument", f"{name} must be a string")
    return value


def required_text(values: JsonObject, name: str) -> str:
    value = text_field(values, name)
    if not value.strip():
        raise DomainError("invalid_argument", f"{name} is required")
    return value


def page_size(values: JsonObject, default: int = 50) -> int:
    value = values.get("page_size", default)
    if not isinstance(value, int) or isinstance(value, bool) or not 1 <= value <= 1000:
        raise DomainError("invalid_argument", "page_size must be between 1 and 1000")
    return value


def integer_field(values: JsonObject, name: str, default: int = 0) -> int:
    value = values.get(name, default)
    # Protobuf JSON represents uint64 as decimal strings.
    if isinstance(value, str) and value.isdecimal():
        return int(value)
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise DomainError("invalid_argument", f"{name} must be a nonnegative integer")
    return value


class RPCContext:
    def __init__(self, app: Application, actor: Principal, metadata: dict[str, str]) -> None:
        self.app, self.actor, self.metadata = app, actor, metadata

    def administrator(self) -> None:
        self.actor.require_instance_management()

    async def scope(
        self,
        namespace_id: str,
        capability: Capability = "read",
        *,
        subject: bool = False,
        request: JsonObject | None = None,
    ) -> CallContext:
        self.actor.require(capability)
        self.actor.require_namespace_scope(namespace_id)
        if not subject:
            self.actor.require_scope(namespace_id)
        supplied = object_field(request, "context") if request else {}
        ctx = CallContext(
            caller_id=self.actor.caller_id,
            namespace_id=namespace_id,
            credential_source_ids=self.actor.source_ids,
            request_id=text_field(supplied, "request_id")
            or self.metadata.get("x-request-id")
            or str(uuid4()),
            locale=self.metadata.get("accept-language", "zh-CN"),
        )
        if subject:
            bound = await self.app.identity.memory_subject(ctx)
            self.actor.require_scope(namespace_id, bound.id)
            ctx = ctx.model_copy(update={})
        return ctx

    def operation_id(self, request: JsonObject, method: str) -> str:
        supplied = object_field(request, "context")
        return text_field(supplied, "idempotency_key") or method + ":" + (
            text_field(supplied, "request_id") or str(uuid4())
        )


def encoded(value: object) -> JsonValue:
    return TypeAdapter(JsonValue).validate_python(
        TypeAdapter(object).dump_python(value, mode="json")
    )
