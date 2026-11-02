"""Immutable creator-document versions in a bound Namespace."""

from __future__ import annotations

from typing import TYPE_CHECKING

from pydantic import TypeAdapter

from .contracts.ego_model import SaveSeed, SeedVersion
from .contracts.imports import ImportDocument
from .operations import encoded, segment

if TYPE_CHECKING:
    from .namespace import NamespaceClient


class SeedResource:
    def __init__(self, namespace: NamespaceClient):
        self.namespace = namespace
        self.path = f"/api/knowledge/namespaces/{segment(namespace.namespace_id)}/ego/seeds"

    async def save(
        self,
        command: SaveSeed | str,
        *,
        instruction: str = "",
        seed_id: str | None = None,
        expected_version: int = 0,
    ) -> SeedVersion:
        selected = (
            command
            if isinstance(command, SaveSeed)
            else SaveSeed(
                documents=(
                    ImportDocument(text=command, filename="seed.txt", source_type="self_seed"),
                ),
                instruction=instruction,
                seed_id=seed_id,
                expected_version=expected_version,
            )
        )
        return SeedVersion.model_validate(
            await self.namespace.request("POST", self.path, body=encoded(selected))
        )

    async def read(self, seed_id: str, *, version: int | None = None) -> SeedVersion:
        return SeedVersion.model_validate(
            await self.namespace.request(
                "GET", self.path + "/" + segment(seed_id), params={"version": version}
            )
        )

    async def history(self, seed_id: str, *, limit: int = 32) -> tuple[SeedVersion, ...]:
        return TypeAdapter(tuple[SeedVersion, ...]).validate_python(
            await self.namespace.request(
                "GET", self.path + "/" + segment(seed_id) + "/history", params={"limit": limit}
            )
        )
