"""Retrieval tuning uses supplied queries and explicit profile publication."""

from ..contracts.tuning import (
    ApplyRetrievalProfile,
    CreateTuning,
    RollbackRetrievalProfile,
    TuningRoundsPage,
    TuningTask,
    TuningTasksPage,
)
from ..operations import encoded, object_value, segment
from ..types import JsonObject
from .jobs import JobClient


class TuningClient(JobClient):
    async def create_tuning(
        self, namespace_id: str,  command: CreateTuning
    ) -> TuningTask:
        return TuningTask.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/retrieval/tuning",
                body=encoded(command),
            )
        )

    async def tuning_tasks(
        self, namespace_id: str,  *, limit: int = 50, cursor: str | None = None
    ) -> TuningTasksPage:
        return TuningTasksPage.model_validate(
            await self.request(
                "GET",
                self.namespace_path(namespace_id) + "/retrieval/tuning",
                params={"limit": limit, "cursor": cursor},
            )
        )

    async def tuning_task(self, namespace_id: str,  task_id: str) -> TuningTask:
        return TuningTask.model_validate(
            await self.request(
                "GET",
                self.namespace_path(namespace_id)
                + f"/retrieval/tuning/{segment(task_id)}",
            )
        )

    async def tuning_rounds(
        self,
        namespace_id: str,
        
        task_id: str,
        *,
        limit: int = 100,
        after: int | None = None,
    ) -> TuningRoundsPage:
        return TuningRoundsPage.model_validate(
            await self.request(
                "GET",
                self.namespace_path(namespace_id)
                + f"/retrieval/tuning/{segment(task_id)}/rounds",
                params={"limit": limit, "after": after},
            )
        )

    async def cancel_tuning(self, namespace_id: str,  task_id: str) -> TuningTask:
        return TuningTask.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id)
                + f"/retrieval/tuning/{segment(task_id)}/cancel",
            )
        )

    async def retrieval_profile(self, namespace_id: str) -> JsonObject:
        return object_value(
            await self.request(
                "GET", self.namespace_path(namespace_id) + "/retrieval/profile"
            )
        )

    async def apply_retrieval_profile(
        self, namespace_id: str,  command: ApplyRetrievalProfile
    ) -> JsonObject:
        return object_value(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/retrieval/profile/apply",
                body=encoded(command),
            )
        )

    async def rollback_retrieval_profile(
        self, namespace_id: str,  command: RollbackRetrievalProfile
    ) -> JsonObject:
        return object_value(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/retrieval/profile/rollback",
                body=encoded(command),
            )
        )
