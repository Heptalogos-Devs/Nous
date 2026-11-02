"""Creator, Social and Motivation tools use the same typed native operations."""

from collections.abc import Callable
from contextlib import AbstractAsyncContextManager

from nous_sdk import NousClient
from nous_sdk.contracts.ego_model import InterpretSeed
from nous_sdk.contracts.motivation import AssessMotivation, MotivationAssessmentsQuery
from nous_sdk.contracts.social import (
    IdentityHypothesis,
    SymbolSelection,
    UnlinkIdentity,
)
from nous_sdk.types import JsonObject

from mcp.server import MCPServer


def register_ego_social(
    server: MCPServer,
    client: Callable[[], AbstractAsyncContextManager[NousClient]],
    namespace_id: str,
    
) -> None:
    @server.tool()
    async def interpret_seed(command: InterpretSeed) -> JsonObject:
        """Preserve creator documents and prepare a typed proposal; returns the durable run/job."""
        async with client() as api:
            return (await api.interpret_seed(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def import_seed(command: InterpretSeed) -> JsonObject:
        """Interpret and adopt creator documents; later versions name the prior adopted run."""
        async with client() as api:
            return (await api.import_seed(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def current_personality() -> str:
        """Read sourced current identity, tendencies and knowledge boundaries as Markdown."""
        async with client() as api:
            return await api.ego_markdown(namespace_id)

    @server.tool()
    async def retry_maintenance(run_id: str, operation_id: str | None = None) -> JsonObject:
        """Continue a failed run/group with its captured sources and saved proposal state."""
        async with client() as api:
            return (
                await api.retry_maintenance(
                    namespace_id,  run_id, operation_id=operation_id
                )
            ).model_dump(mode="json")

    @server.tool()
    async def propose_identity(command: IdentityHypothesis) -> JsonObject:
        """Record a source-backed possible person correspondence without confirming account ownership."""
        async with client() as api:
            return (await api.propose_identity(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def unlink_identity(command: UnlinkIdentity) -> JsonObject:
        """Correct an account correspondence using an exact person revision and source evidence."""
        async with client() as api:
            return (await api.unlink_identity(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def understand_symbol(command: SymbolSelection) -> JsonObject:
        """Read applicable text/media meanings for the actual audience and social context."""
        async with client() as api:
            return (await api.understand_symbol(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def expression_candidates(command: SymbolSelection) -> JsonObject:
        """Read expression candidates honoring audience preferences; sending remains the host's action."""
        async with client() as api:
            return (await api.expression_candidates(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def assess_motivation(command: AssessMotivation) -> JsonObject:
        """Evaluate a desired condition against exact evidence, reporting actual interval gaps."""
        async with client() as api:
            return (await api.assess_motivation(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def motivation_assessment(assessment_id: str) -> JsonObject:
        """Read one saved assessment, its exact original condition, evidence and model judgments."""
        async with client() as api:
            return (
                await api.motivation_assessment(namespace_id,  assessment_id)
            ).model_dump(mode="json")

    @server.tool()
    async def motivation_assessments(query: MotivationAssessmentsQuery) -> JsonObject:
        """Page saved assessment history, optionally for an exact original condition revision."""
        async with client() as api:
            return (await api.motivation_assessments(namespace_id,  query)).model_dump(
                mode="json"
            )
