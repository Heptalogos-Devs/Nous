"""Typed Entity identity changes for the bound authorized Subject."""

from collections.abc import Callable
from contextlib import AbstractAsyncContextManager

from nous_sdk import NousClient
from nous_sdk.contracts import MergeEntities, SplitEntity
from nous_sdk.types import JsonObject

from mcp.server import MCPServer


def register_entities(
    server: MCPServer,
    client: Callable[[], AbstractAsyncContextManager[NousClient]],
    namespace_id: str,
    
) -> None:
    @server.tool()
    async def merge_entities(command: MergeEntities) -> list[JsonObject]:
        """Merge same-kind Entity identities, migrating current references and person claims."""
        async with client() as api:
            return [
                record.model_dump(mode="json")
                for record in await api.merge_entities(namespace_id,  command)
            ]

    @server.tool()
    async def split_entity(command: SplitEntity) -> list[JsonObject]:
        """Split an Entity using complete sourced field, claim and consumer assignments."""
        async with client() as api:
            return [
                record.model_dump(mode="json")
                for record in await api.split_entity(namespace_id,  command)
            ]
