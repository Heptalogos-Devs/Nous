"""Typed clients for actual growth runs, review and DBOS repeat policies."""

from uuid import uuid4

from pydantic import TypeAdapter

from ..contracts.maintenance import (
    CreateMaintenanceSchedule,
    MaintenanceRun,
    MaintenanceSchedule,
    MaintenanceSubmission,
    ReviewMaintenance,
    StartMaintenance,
)
from ..operations import encoded, segment
from .social import SocialClient


class MaintenanceClient(SocialClient):
    async def start_maintenance(
        self, namespace_id: str, subject_id: str, command: StartMaintenance
    ) -> MaintenanceSubmission:
        return MaintenanceSubmission.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/maintenance/runs",
                body=encoded(command),
            )
        )

    async def maintenance_runs(
        self, namespace_id: str, subject_id: str, *, limit: int = 50
    ) -> tuple[MaintenanceRun, ...]:
        return TypeAdapter(tuple[MaintenanceRun, ...]).validate_python(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id) + "/maintenance/runs",
                params={"limit": limit},
            )
        )

    async def maintenance_run(
        self, namespace_id: str, subject_id: str, run_id: str
    ) -> MaintenanceRun:
        return MaintenanceRun.model_validate(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id)
                + f"/maintenance/runs/{segment(run_id)}",
            )
        )

    async def review_maintenance(
        self, namespace_id: str, subject_id: str, run_id: str, command: ReviewMaintenance
    ) -> MaintenanceRun:
        return MaintenanceRun.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id)
                + f"/maintenance/runs/{segment(run_id)}/review",
                body=encoded(command),
            )
        )

    async def create_maintenance_schedule(
        self, namespace_id: str, subject_id: str, command: CreateMaintenanceSchedule
    ) -> MaintenanceSchedule:
        return MaintenanceSchedule.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/maintenance/schedules",
                body=encoded(command),
            )
        )

    async def maintenance_schedules(
        self, namespace_id: str, subject_id: str
    ) -> tuple[MaintenanceSchedule, ...]:
        return TypeAdapter(tuple[MaintenanceSchedule, ...]).validate_python(
            await self.request(
                "GET", self.subject_path(namespace_id, subject_id) + "/maintenance/schedules"
            )
        )

    async def stop_maintenance_schedule(
        self,
        namespace_id: str,
        subject_id: str,
        schedule_id: str,
        *,
        operation_id: str | None = None,
    ) -> MaintenanceSchedule:
        return MaintenanceSchedule.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id)
                + f"/maintenance/schedules/{segment(schedule_id)}/stop",
                body={"operation_id": operation_id or str(uuid4())},
            )
        )
