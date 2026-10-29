"""MCP growth tools retain the connection's namespace, subject and caller grants."""

from collections.abc import Callable
from contextlib import AbstractAsyncContextManager

from nous_wave import NousClient
from nous_wave.contracts.maintenance import (
    CreateMaintenanceSchedule,
    ReviewMaintenance,
    StartMaintenance,
)
from nous_wave.types import JsonObject

from mcp.server import MCPServer


def register_maintenance(
    server: MCPServer,
    client: Callable[[], AbstractAsyncContextManager[NousClient]],
    namespace_id: str,
    subject_id: str,
) -> None:
    @server.tool()
    async def start_maintenance(command: StartMaintenance) -> JsonObject:
        """Freeze supported knowledge and generate a reviewable proposal through the configured model.

        Generation is durable. Read its job and proposal before explicitly adopting
        or rejecting; generation itself never adopts the proposed cognition.
        """
        async with client() as api:
            return (await api.start_maintenance(namespace_id, subject_id, command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def maintenance_runs(limit: int = 50) -> JsonObject:
        """List this subject's reviewable growth runs with currently authorized source content."""
        async with client() as api:
            return {
                "items": [
                    run.model_dump(mode="json")
                    for run in await api.maintenance_runs(namespace_id, subject_id, limit=limit)
                ]
            }

    @server.tool()
    async def read_maintenance(run_id: str) -> JsonObject:
        """Read exact proposal actions/catalog/revision and review facts; read_job supplies execution state."""
        async with client() as api:
            return (await api.maintenance_run(namespace_id, subject_id, run_id)).model_dump(
                mode="json"
            )

    @server.tool()
    async def review_maintenance(run_id: str, command: ReviewMaintenance) -> JsonObject:
        """Explicitly adopt all proposed actions atomically, or reject, using expected_revision.

        Exact evidence, local references, current revision and Journal knowledge
        cutoff are checked by the cognition owner; no partial adoption is reported.
        """
        async with client() as api:
            return (
                await api.review_maintenance(namespace_id, subject_id, run_id, command)
            ).model_dump(mode="json")

    @server.tool()
    async def create_maintenance_schedule(command: CreateMaintenanceSchedule) -> JsonObject:
        """Create a DBOS repeat policy for reviewable growth; requires manage permission.

        Each occurrence generates a new proposal. It never automatically adopts
        one, and missed occurrences are not automatically backfilled.
        """
        async with client() as api:
            return (
                await api.create_maintenance_schedule(namespace_id, subject_id, command)
            ).model_dump(mode="json")

    @server.tool()
    async def maintenance_schedules() -> JsonObject:
        """Read this subject's saved repeat policies and explicit stop facts."""
        async with client() as api:
            return {
                "items": [
                    policy.model_dump(mode="json")
                    for policy in await api.maintenance_schedules(namespace_id, subject_id)
                ]
            }

    @server.tool()
    async def stop_maintenance_schedule(
        schedule_id: str, operation_id: str | None = None
    ) -> JsonObject:
        """Stop future repeat admissions through the public DBOS scheduler; requires manage permission."""
        async with client() as api:
            return (
                await api.stop_maintenance_schedule(
                    namespace_id, subject_id, schedule_id, operation_id=operation_id
                )
            ).model_dump(mode="json")
