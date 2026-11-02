"""Configuration resources expose complete TOML files and registered key views."""

from __future__ import annotations

from typing import TYPE_CHECKING

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
                "GET", "/api/manage/configuration/files/current", params=self.parameters()
            )
        )
        return TypeAdapter(str).validate_python(value["content"])

    async def replace(self, content: str) -> dict[str, JsonValue]:
        return TypeAdapter(dict[str, JsonValue]).validate_python(
            await self.client.request(
                "PUT",
                "/api/manage/configuration/files/current",
                params=self.parameters(),
                body={"content": content},
            )
        )

    async def reload(self) -> dict[str, JsonValue]:
        return TypeAdapter(dict[str, JsonValue]).validate_python(
            await self.client.request(
                "POST",
                "/api/manage/configuration/reload",
                params=self.parameters(),
                body={"scope": self.scope},
            )
        )

    async def get(self, key: str) -> dict[str, JsonValue]:
        return TypeAdapter(dict[str, JsonValue]).validate_python(
            await self.client.request(
                "GET", f"/api/manage/configuration/{segment(key)}", params=self.parameters()
            )
        )
