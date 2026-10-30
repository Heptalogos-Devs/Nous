"""Namespace-bound MCP tools consume the same public service contracts."""

import json
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Literal
from uuid import uuid4

from nous_sdk import NousClient
from nous_sdk.amemorix import AMemorixAdapter
from nous_sdk.amemorix.contracts import ProfileAdminInput
from nous_sdk.contracts import AdmitFiles, AdoptCognition, DomainError, QueryRequest
from nous_sdk.contracts.context import (
    ContextAdmissionCommand,
    ContextPrepare,
    ContextReport,
)
from nous_sdk.errors import NousError as ClientError
from nous_sdk.presentation import query_text
from nous_sdk.types import JsonObject
from pydantic import JsonValue, TypeAdapter

from mcp.server import MCPServer
from mcp.server.mcpserver.exceptions import ResourceError, ToolError
from mcp.types import CallToolResult, TextContent
from nous.platform.errors import NousError
from nous.platform.observability import configure_logging

from .maintenance import register_maintenance
from .management import register_management


def run(
    instance: Path,
    url: str,
    namespace_id: str,
    subject_id: str,
    credential_file: Path | None = None,
) -> None:
    mcp = MCPServer("Nous")
    configure_logging("INFO")
    token_path = credential_file or instance / "credentials/admin.token"

    @asynccontextmanager
    async def client() -> AsyncGenerator[NousClient]:
        try:
            async with NousClient(
                url, credential_file=token_path, namespace_id=namespace_id, subject_id=subject_id
            ) as api:
                yield api
        except (ClientError, DomainError, NousError) as error:
            details = error.details if isinstance(error, DomainError) else error.context
            raise ToolError(
                json.dumps(
                    {
                        "error": {
                            "code": error.code,
                            "message": str(error),
                            "context": details,
                            "trace_id": error.trace_id if isinstance(error, ClientError) else None,
                        }
                    },
                    ensure_ascii=False,
                )
            ) from error

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
    async def memory_profile_admin(command: ProfileAdminInput) -> JsonObject:
        """Read profiles or explicitly change aliases, profile text and exact evidence."""
        values = command.model_dump(mode="json")
        if not values.get("operation_id"):
            values["operation_id"] = str(uuid4())
        return await call("memory_profile_admin", values)

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
    async def memory_graph_admin(
        query: str = "",
        limit: int = 50,
        action: Literal[
            "search",
            "get_graph",
            "node_detail",
            "edge_detail",
            "create_node",
            "rename_node",
            "delete_node",
            "create_edge",
            "update_edge_weight",
            "delete_edge",
        ] = "search",
        arguments: dict[str, JsonValue] | None = None,
    ) -> JsonObject:
        """Read graph/search/details or apply source-backed create, rename, weight and delete actions."""
        return await call(
            "memory_graph_admin",
            {**(arguments or {}), "action": action, "query": query, "limit": limit},
        )

    @mcp.tool()
    async def memory_source(target: str) -> JsonObject:
        """Expand a precise cognitive reference's permitted provenance."""
        async with client() as api:
            return await AMemorixAdapter(
                api, namespace_id=namespace_id, subject_id=subject_id
            ).memory_source(target)

    @mcp.tool(structured_output=False)
    async def query_cognition(command: QueryRequest) -> CallToolResult:
        """Query cognition and media. Read exact references in text; structured content holds the full result."""
        async with client() as api:
            result = await api.query_cognition(namespace_id, subject_id, command)
            return CallToolResult(
                content=[TextContent(type="text", text=query_text(result))],
                structured_content=result.model_dump(mode="json"),
            )

    @mcp.tool()
    async def service_status() -> JsonObject:
        """Read the running instance status."""
        async with client() as api:
            return TypeAdapter(JsonObject).validate_python(await api.request("GET", "/api/status"))

    @mcp.tool()
    async def resolve_reference(reference: str, as_of_commit: int | None = None) -> JsonObject:
        """Resolve a readable or canonical reference within the configured subject."""
        async with client() as api:
            return (
                await api.resolve_reference(subject_id, reference, as_of_commit=as_of_commit)
            ).model_dump(mode="json")

    @mcp.tool()
    async def admit_documents(command: AdmitFiles) -> JsonObject:
        """Preserve real documents and form cognition under the supplied completion policy."""
        async with client() as api:
            return (await api.admit_documents(namespace_id, subject_id, command)).model_dump(
                mode="json"
            )

    @mcp.tool()
    async def adopt_cognition(command: AdoptCognition) -> JsonObject:
        """Adopt structured cognition supported by exact, already observed evidence."""
        async with client() as api:
            return (await api.adopt_cognition(namespace_id, subject_id, command)).model_dump(
                mode="json"
            )

    @mcp.tool()
    async def prepare_context(command: ContextPrepare) -> JsonObject:
        """Prepare scoped cognition for one host model invocation; this does not admit its use."""
        async with client() as api:
            return (await api.prepare_context(namespace_id, subject_id, command)).model_dump(
                mode="json"
            )

    @mcp.tool()
    async def admit_context(command: ContextAdmissionCommand) -> JsonObject:
        """Record the host's applied segments, rendered digest and complete input measurement."""
        async with client() as api:
            return (await api.admit_managed_context(namespace_id, subject_id, command)).model_dump(
                mode="json"
            )

    @mcp.tool()
    async def report_context(command: ContextReport) -> JsonObject:
        """Report an admitted invocation's actual outcome and meaningful cognition uses."""
        async with client() as api:
            return (await api.report_managed_context(namespace_id, subject_id, command)).model_dump(
                mode="json"
            )

    @mcp.tool()
    async def read_job(job_id: str) -> JsonObject:
        """Read a durable import or context preparation job in the bound subject."""
        async with client() as api:
            return (
                await api.job(job_id, namespace_id=namespace_id, subject_id=subject_id)
            ).model_dump(mode="json")

    @mcp.resource("nous://query/manual")
    async def query_manual() -> str:
        """The running service's NousQL syntax and retrieval domains."""
        try:
            async with client() as api:
                return await api.query_manual()
        except ToolError as error:
            raise ResourceError(str(error)) from error

    register_management(mcp, client, namespace_id, subject_id)
    register_maintenance(mcp, client, namespace_id, subject_id)
    mcp.run(transport="stdio")
