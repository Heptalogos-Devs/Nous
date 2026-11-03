"""Typed clients for actual growth runs, review and DBOS repeat policies."""

from uuid import uuid4

from pydantic import TypeAdapter

from ..contracts.ego_model import InterpretSeed
from ..contracts.maintenance import (
    CreateMaintenanceSchedule,
    EditMaintenanceProposal,
    MaintenanceRun,
    MaintenanceSchedule,
    MaintenanceSubmission,
    ReviewMaintenance,
    StartMaintenance,
)
from ..operations import encoded, segment
from .social import SocialClient


class MaintenanceClient(SocialClient):
    async def edit_maintenance(
        self, namespace_id: str,  run_id: str, command: EditMaintenanceProposal
    ) -> MaintenanceRun:
        return MaintenanceRun.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id)
                + f"/maintenance/runs/{segment(run_id)}/edit",
                body=encoded(command),
            )
        )

    async def retry_maintenance(
        self, namespace_id: str,  run_id: str, *, operation_id: str | None = None
    ) -> MaintenanceSubmission:
        return MaintenanceSubmission.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id)
                + f"/maintenance/runs/{segment(run_id)}/retry",
                body={"operation_id": operation_id or str(uuid4())},
            )
        )

    async def interpret_seed(
        self, namespace_id: str,  command: InterpretSeed
    ) -> MaintenanceSubmission:
        return MaintenanceSubmission.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/ego/seed/interpret",
                body=encoded(command),
            )
        )

    async def import_seed(
        self, namespace_id: str,  command: InterpretSeed
    ) -> MaintenanceRun:
        return MaintenanceRun.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/ego/seed/import",
                body=encoded(command),
            )
        )

    async def ego_markdown(self, namespace_id: str) -> str:
        return TypeAdapter(dict[str, str]).validate_python(
            await self.request(
                "GET",
                self.namespace_path(namespace_id) + "/ego/profile/view",
            )
        )["markdown"]

    async def start_maintenance(
        self, namespace_id: str,  command: StartMaintenance
    ) -> MaintenanceSubmission:
        return MaintenanceSubmission.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/maintenance/runs",
                body=encoded(command),
            )
        )

    async def maintenance_runs(
        self, namespace_id: str,  *, limit: int = 50
    ) -> tuple[MaintenanceRun, ...]:
        return TypeAdapter(tuple[MaintenanceRun, ...]).validate_python(
            await self.request(
                "GET",
                self.namespace_path(namespace_id) + "/maintenance/runs",
                params={"limit": limit},
            )
        )

    async def maintenance_run(
        self, namespace_id: str,  run_id: str
    ) -> MaintenanceRun:
        return MaintenanceRun.model_validate(
            await self.request(
                "GET",
                self.namespace_path(namespace_id)
                + f"/maintenance/runs/{segment(run_id)}",
            )
        )

    async def review_maintenance(
        self, namespace_id: str,  run_id: str, command: ReviewMaintenance
    ) -> MaintenanceRun:
        return MaintenanceRun.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id)
                + f"/maintenance/runs/{segment(run_id)}/review",
                body=encoded(command),
            )
        )

    async def create_maintenance_schedule(
        self, namespace_id: str,  command: CreateMaintenanceSchedule
    ) -> MaintenanceSchedule:
        return MaintenanceSchedule.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/maintenance/schedules",
                body=encoded(command),
            )
        )

    async def maintenance_schedules(
        self, namespace_id: str
    ) -> tuple[MaintenanceSchedule, ...]:
        return TypeAdapter(tuple[MaintenanceSchedule, ...]).validate_python(
            await self.request(
                "GET", self.namespace_path(namespace_id) + "/maintenance/schedules"
            )
        )

    async def stop_maintenance_schedule(
        self,
        namespace_id: str,
        
        schedule_id: str,
        *,
        operation_id: str | None = None,
    ) -> MaintenanceSchedule:
        return MaintenanceSchedule.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id)
                + f"/maintenance/schedules/{segment(schedule_id)}/stop",
                body={"operation_id": operation_id or str(uuid4())},
            )
        )
