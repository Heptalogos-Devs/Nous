"""Namespace-bound MCP tools consume the same public service contracts."""

from pathlib import Path
from typing import Literal

from nous_wave import NousClient
from nous_wave.amemorix import AMemorixAdapter
from nous_wave.contracts import QueryRequest
from nous_wave.types import JsonObject
from pydantic import JsonValue, TypeAdapter

from mcp.server import MCPServer


def run(
    instance: Path,
    url: str,
    namespace_id: str,
    subject_id: str,
    credential_file: Path | None = None,
) -> None:
    mcp = MCPServer("Nous Wave")
    token_path = credential_file or instance / "credentials/admin.token"

    def client() -> NousClient:
        return NousClient(url, credential_file=token_path)

    async def call(tool: str, arguments: dict[str, JsonValue]) -> JsonObject:
        async with client() as api:
            adapter = AMemorixAdapter(api, namespace_id=namespace_id, subject_id=subject_id)
            return await adapter.call(tool, arguments)

    @mcp.tool()
    async def search_memory(
        query: str,
        limit: int = 10,
        mode: Literal["search", "time", "hybrid", "episode", "aggregate"] = "search",
        chat_id: str = "",
        person_id: str = "",
    ) -> JsonObject:
        """Search the configured subject's memories and exact provenance."""
        return await call(
            "search_memory",
            {
                "query": query,
                "limit": limit,
                "mode": mode,
                "chat_id": chat_id,
                "person_id": person_id,
            },
        )

    @mcp.tool()
    async def ingest_text(
        external_id: str,
        text: str,
        source_type: str,
        chat_id: str = "",
        person_ids: tuple[str, ...] = (),
    ) -> JsonObject:
        """Adopt actual text with stable external identity and source attribution."""
        return await call(
            "ingest_text",
            {
                "external_id": external_id,
                "text": text,
                "source_type": source_type,
                "chat_id": chat_id,
                "person_ids": list(person_ids),
            },
        )

    @mcp.tool()
    async def ingest_summary(
        external_id: str, text: str, chat_id: str, participants: tuple[str, ...] = ()
    ) -> JsonObject:
        """Adopt a reported conversation summary with its actual participants."""
        return await call(
            "ingest_summary",
            {
                "external_id": external_id,
                "text": text,
                "chat_id": chat_id,
                "participants": list(participants),
            },
        )

    @mcp.tool()
    async def get_person_profile(person_id: str, chat_id: str = "", limit: int = 10) -> JsonObject:
        """Read the configured subject's supported knowledge about a person."""
        return await call(
            "get_person_profile", {"person_id": person_id, "chat_id": chat_id, "limit": limit}
        )

    @mcp.tool()
    async def maintain_memory(
        action: Literal[
            "reinforce", "weaken", "protect", "restore", "freeze", "forget", "recycle_bin"
        ],
        target: str = "",
        hours: float | None = None,
        reason: str = "",
    ) -> JsonObject:
        """Apply a memory lifecycle change or read the recycle bin."""
        from uuid import uuid4

        return await call(
            "maintain_memory",
            {
                "action": action,
                "target": target,
                "hours": hours,
                "reason": reason,
                "operation_id": str(uuid4()),
            },
        )

    @mcp.tool()
    async def memory_stats() -> JsonObject:
        """Read actual cognitive and source counts."""
        return await call("memory_stats", {})

    @mcp.tool()
    async def memory_source(target: str) -> JsonObject:
        """Expand a precise cognitive reference's permitted provenance."""
        async with client() as api:
            return await AMemorixAdapter(
                api, namespace_id=namespace_id, subject_id=subject_id
            ).memory_source(target)

    @mcp.tool()
    async def query_cognition(command: QueryRequest) -> JsonObject:
        """Run NousQL, multi-domain and joint text/media queries within the bound subject."""
        async with client() as api:
            return (await api.query_cognition(namespace_id, subject_id, command)).model_dump(
                mode="json"
            )

    @mcp.tool()
    async def service_status() -> JsonObject:
        """Read the running instance status."""
        async with client() as api:
            return TypeAdapter(JsonObject).validate_python(await api.request("GET", "/api/status"))

    mcp.run(transport="stdio")
