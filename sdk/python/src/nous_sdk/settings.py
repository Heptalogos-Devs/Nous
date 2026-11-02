"""Configuration resources expose complete TOML files and registered key views."""

from __future__ import annotations

from typing import TYPE_CHECKING
from uuid import uuid4

from pydantic import JsonValue, TypeAdapter

from .operations import segment

if TYPE_CHECKING:
    from .client import NousClient


class SettingsResource:
    def __init__(self, client: NousClient, namespace_id: str | None = None):
        self.client, self.namespace_id = client, namespace_id
        self.scope = "namespace" if namespace_id is not None else "instance"

    def parameters(self) -> dict[str, str]:
        return {"scope": self.scope, "namespace_id": self.namespace_id or "instance"}

    async def export(self) -> str:
        value = TypeAdapter(dict[str, JsonValue]).validate_python(
            await self.client.request(
                "GET", "/api/v1/configuration/files/current", params=self.parameters()
            )
        )
        return TypeAdapter(str).validate_python(value["content"])

    async def replace(
        self, content: str, *, operation_id: str | None = None
    ) -> dict[str, JsonValue]:
        return TypeAdapter(dict[str, JsonValue]).validate_python(
            await self.client.request(
                "PUT",
                "/api/v1/configuration/files/current",
                params=self.parameters(),
                body={"content": content, "operation_id": operation_id or str(uuid4())},
            )
        )

    async def reload(self, *, operation_id: str | None = None) -> dict[str, JsonValue]:
        return TypeAdapter(dict[str, JsonValue]).validate_python(
            await self.client.request(
                "POST",
                "/api/v1/configuration/reload",
                params=self.parameters(),
                body={"scope": self.scope, "operation_id": operation_id or str(uuid4())},
            )
        )

    async def get(self, key: str) -> dict[str, JsonValue]:
        return TypeAdapter(dict[str, JsonValue]).validate_python(
            await self.client.request(
                "GET", f"/api/v1/configuration/{segment(key)}", params=self.parameters()
            )
        )

    async def history(self, key: str, *, limit: int = 100) -> dict[str, JsonValue]:
        return TypeAdapter(dict[str, JsonValue]).validate_python(
            await self.client.request(
                "GET",
                f"/api/v1/configuration/{segment(key)}/history",
                params={**self.parameters(), "limit": limit},
            )
        )
