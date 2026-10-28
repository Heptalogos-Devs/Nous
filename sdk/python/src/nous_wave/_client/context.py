"""Typed context client operations."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from datetime import datetime
from typing import Literal
from uuid import uuid4

from pydantic import JsonValue

from ..contracts.context import ContextReport, ContextReportResult, ContextUse
from ..operations import encoded, object_value
from ..source_scope import SourceReadScope
from ..types import (
    JsonObject,
)
from .knowledge import KnowledgeClient


class ContextClient(KnowledgeClient):
    async def report_context(
        self,
        admission_id: str,
        *,
        namespace_id: str,
        subject_id: str,
        invocation_id: str,
        outcome: Literal["completed", "failed", "cancelled"],
        occurred_at: datetime,
        uses: Sequence[ContextUse] = (),
    ) -> ContextReportResult:
        from ..managed import event_id

        return await self.report_managed_context(
            namespace_id,
            subject_id,
            ContextReport(
                operation_id=event_id("report", invocation_id, admission_id),
                admission_id=admission_id,
                invocation_id=invocation_id,
                outcome=outcome,
                occurred_at=occurred_at,
                uses=tuple(uses),
            ),
        )

    def managed(
        self,
        *,
        namespace_id: str | None = None,
        subject_id: str,
        session_id: str,
        consumer_id: str,
        instance_id: str,
        source_read: SourceReadScope | Mapping[str, JsonValue] | None = None,
    ):
        from ..managed import ManagedBinding

        namespace = namespace_id or self.namespace_id
        return ManagedBinding(
            self,
            namespace_id=namespace,
            subject_id=subject_id,
            session_id=session_id,
            consumer_id=consumer_id,
            instance_id=instance_id,
            source_read=source_read,
        )

    async def contexts(self, request: Mapping[str, JsonValue]) -> JsonObject:
        from ..contracts.context import (
            ContextAdmissionCommand,
            ContextPrepare,
            ContextReport,
            OpenContextSession,
        )

        values = object_value(request.get("request", {}))
        subject_id = values.pop("subject_id", None) or request.get("subject_id") or self.subject_id
        namespace_id = (
            values.pop("namespace_id", None) or request.get("namespace_id") or self.namespace_id
        )
        if not isinstance(subject_id, str):
            raise ValueError("Context operation requires subject_id")
        if not isinstance(namespace_id, str):
            namespace_id = (await self.subject(subject_id)).namespace_id
        operation_id = request.get("operation_id") or str(uuid4())
        if not isinstance(operation_id, str):
            raise ValueError("operation_id must be text")
        action = request.get("action")
        if action == "open_session":
            return {
                "result": encoded(
                    await self.open_context_session(
                        namespace_id,
                        subject_id,
                        OpenContextSession.model_validate({"operation_id": operation_id, **values}),
                    )
                )
            }
        if action == "prepare":
            prepared = await self.prepare_context(
                namespace_id,
                subject_id,
                ContextPrepare.model_validate({"operation_id": operation_id, **values}),
            )
            return {"result": {"preparation_id": prepared.id}, "job_ids": []}
        identity = request.get("id")
        if action in {"preparation", "cancel_preparation"}:
            if not isinstance(identity, str):
                raise ValueError("Context operation requires preparation ID")
            result = (
                await self.context_preparation(namespace_id, subject_id, identity)
                if action == "preparation"
                else await self.cancel_context_preparation(
                    namespace_id, subject_id, identity, operation_id
                )
            )
            return object_value(encoded(result))
        if action == "admit":
            admission = await self.admit_managed_context(
                namespace_id,
                subject_id,
                ContextAdmissionCommand.model_validate({"operation_id": operation_id, **values}),
            )
            return {"result": encoded(admission)}
        if action == "report":
            return {
                "result": encoded(
                    await self.report_managed_context(
                        namespace_id,
                        subject_id,
                        ContextReport.model_validate({"operation_id": operation_id, **values}),
                    )
                )
            }
        if action == "close_session":
            if not isinstance(identity, str):
                raise ValueError("Close requires session ID")
            return {
                "result": encoded(
                    await self.close_context_session(
                        namespace_id, subject_id, identity, operation_id
                    )
                )
            }
        raise ValueError("Unknown context operation")
