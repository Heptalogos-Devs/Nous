"""Scoped Episode tools share typed HTTP/SDK operations and the actual A facade."""

from collections.abc import Callable
from contextlib import AbstractAsyncContextManager
from uuid import uuid4

from nous_wave import NousClient
from nous_wave.amemorix import AMemorixAdapter
from nous_wave.amemorix.episodes import EpisodeAdminInput
from nous_wave.contracts import CognitiveRef
from nous_wave.contracts.episodes import EpisodeQuery, RebuildEpisodes
from nous_wave.types import JsonObject

from mcp.server import MCPServer


def register_episodes(
    server: MCPServer,
    client: Callable[[], AbstractAsyncContextManager[NousClient]],
    namespace_id: str,
    subject_id: str,
) -> None:
    @server.tool()
    async def query_episodes(command: EpisodeQuery) -> list[JsonObject]:
        """Read scoped Episodes with their actual ordered members, summaries and sources."""
        async with client() as api:
            return [
                view.model_dump(mode="json")
                for view in await api.query_episodes(namespace_id, subject_id, command)
            ]

    @server.tool()
    async def read_episode(reference: CognitiveRef, paragraph_limit: int = 100) -> JsonObject:
        """Read an exact Episode revision and its ordered retained members."""
        async with client() as api:
            return (
                await api.episode(
                    namespace_id, subject_id, reference, paragraph_limit=paragraph_limit
                )
            ).model_dump(mode="json")

    @server.tool()
    async def episode_history(episode_id: str, limit: int = 100) -> list[JsonObject]:
        """Read actual retained Episode revisions through the cognitive history owner."""
        async with client() as api:
            return [
                record.model_dump(mode="json")
                for record in await api.episode_history(
                    namespace_id, subject_id, episode_id, limit=limit
                )
            ]

    @server.tool()
    async def rebuild_episodes(command: RebuildEpisodes) -> JsonObject:
        """Admit a durable complete-source rebuild; model generation is explicit."""
        async with client() as api:
            return (await api.rebuild_episodes(namespace_id, subject_id, command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def memory_episode_admin(command: EpisodeAdminInput) -> JsonObject:
        """Use the real MaiBot Episode list/get/status/rebuild/process_sources contract."""
        values = command.model_dump(mode="json")
        if command.action == "rebuild" and not command.operation_id:
            values["operation_id"] = str(uuid4())
        async with client() as api:
            return await AMemorixAdapter(
                api, namespace_id=namespace_id, subject_id=subject_id
            ).call("memory_episode_admin", values)
