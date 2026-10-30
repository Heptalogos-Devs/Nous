"""Fact tools preserve the same patch, receipt and source rules as HTTP and the SDK."""

from collections.abc import Callable
from contextlib import AbstractAsyncContextManager
from uuid import uuid4

from nous_sdk import NousClient
from nous_sdk.amemorix import AMemorixAdapter
from nous_sdk.amemorix.facts import FactAdminInput
from nous_sdk.contracts.facts import FactChange, FactQuery
from nous_sdk.types import JsonObject

from mcp.server import MCPServer


def register_facts(
    server: MCPServer,
    client: Callable[[], AbstractAsyncContextManager[NousClient]],
    namespace_id: str,
    subject_id: str,
) -> None:
    @server.tool()
    async def change_fact(command: FactChange) -> JsonObject:
        """Apply one source-backed claim assertion, classification, withdrawal or evidence update."""
        async with client() as api:
            return (await api.change_fact(namespace_id, subject_id, command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def query_facts(command: FactQuery) -> list[JsonObject]:
        """Query actual current claim heads, filtering state and declared validity."""
        async with client() as api:
            return [
                record.model_dump(mode="json")
                for record in await api.query_facts(namespace_id, subject_id, command)
            ]

    @server.tool()
    async def read_fact(claim_id: str) -> JsonObject:
        """Read the canonical claim, evidence and retained transitions."""
        async with client() as api:
            return (await api.fact(namespace_id, subject_id, claim_id)).model_dump(mode="json")

    @server.tool()
    async def memory_fact_admin(command: FactAdminInput) -> JsonObject:
        """Consume the actual MaiBot get/list/create/update/retract/restore actions."""
        values = command.model_dump(mode="json", exclude_unset=True)
        if (
            command.action in {"create", "update", "retract", "restore"}
            and not command.operation_id
        ):
            values["operation_id"] = str(uuid4())
        async with client() as api:
            return await AMemorixAdapter(
                api, namespace_id=namespace_id, subject_id=subject_id
            ).call("memory_fact_admin", values)
