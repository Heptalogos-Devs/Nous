"""Typed context client operations."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from datetime import datetime
from typing import Literal

from pydantic import JsonValue

from ..contracts.context import (
    ContextInvocationOutput,
    ContextReport,
    ContextReportResult,
    ContextTrack,
    ContextTrackState,
    ContextUse,
)
from ..operations import encoded, object_value
from ..source_scope import SourceReadScope
from .sources import SourceClient


class ContextClient(SourceClient):
    async def context_track(
        self, namespace_id: str, subject_id: str, command: ContextTrack
    ) -> ContextTrackState:
        result = await self.request(
            "POST",
            self.subject_path(namespace_id, subject_id) + "/contexts/track",
            body=encoded(command),
        )
        return ContextTrackState.model_validate(result)

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
        output: ContextInvocationOutput | None = None,
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
                output=output,
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

    async def contexts(self, request: Mapping[str, JsonValue]) -> JsonValue:
        from pydantic import TypeAdapter

        from ..contracts.context import (
            ChangeContextSession,
            ChangeResidents,
            ChangeWorkContext,
            ContextAdmissionCommand,
            ContextPrepare,
            ContextReport,
            OpenContextSession,
        )
        from .requests import operation_identity, operation_values, request_scope, required_text

        values = operation_values(request)
        source_read = values.pop("source_read", None)
        if source_read is not None:
            scoped = self.with_source_read(SourceReadScope.model_validate(source_read))
            return await scoped.contexts({"action": request.get("action"), "request": values})
        namespace, subject = await request_scope(self, values)
        action = request.get("action")
        operation_id = operation_identity(values)
        if action == "track":
            return encoded(
                await self.context_track(namespace, subject, ContextTrack.model_validate(values))
            )
        if action == "open_session":
            if "native_session" in values:
                values["session_id"] = values.pop("native_session")
            if "parent_session" in values:
                values["parent_session_id"] = values.pop("parent_session")
            return {
                "result": encoded(
                    await self.open_context_session(
                        namespace,
                        subject,
                        OpenContextSession.model_validate({"operation_id": operation_id, **values}),
                    )
                )
            }
        if action == "read_session":
            return encoded(
                await self.context_session(namespace, subject, required_text(values, "id"))
            )
        if action == "close_session":
            return {
                "result": encoded(
                    await self.close_context_session(
                        namespace, subject, required_text(values, "id"), operation_id
                    )
                )
            }
        if action == "change_session":
            identity = required_text(values, "id")
            change = object_value(values.pop("change", {}))
            if "active_work_context" in change:
                change["active_work_context_id"] = change.pop("active_work_context")
            return {
                "result": encoded(
                    await self.change_context_session(
                        namespace,
                        subject,
                        ChangeContextSession.model_validate(
                            {
                                "operation_id": operation_id,
                                "session_id": identity,
                                **values,
                                **change,
                            }
                        ),
                    )
                )
            }
        if action == "residents":
            session = await self.context_session(
                namespace, subject, required_text(values, "session_id")
            )
            include_evicted = TypeAdapter(bool).validate_python(
                values.pop("include_evicted", False)
            )
            return [
                encoded(item)
                for item in session.residents
                if include_evicted or item.state != "evicted"
            ]
        if action == "change_residents":
            change = object_value(values.pop("change", {}))
            change_action = change.pop("action", None)
            if "items" in change:
                items = TypeAdapter(list[dict[str, JsonValue]]).validate_python(change.pop("items"))
                for item in items:
                    if "reason" in item:
                        item["entry_reason"] = item.pop("reason")
                change["residents"] = TypeAdapter(JsonValue).validate_python(items)
            return {
                "result": encoded(
                    await self.change_context_residents(
                        namespace,
                        subject,
                        ChangeResidents.model_validate(
                            {
                                "operation_id": operation_id,
                                **values,
                                **change,
                                "action": change_action,
                            }
                        ),
                    )
                )
            }
        if action == "create_work":
            return {
                "result": encoded(
                    await self.change_work_context(
                        namespace,
                        subject,
                        ChangeWorkContext.model_validate(
                            {"operation_id": operation_id, "action": "create", **values}
                        ),
                    )
                )
            }
        if action == "read_work":
            return encoded(await self.work_context(namespace, subject, required_text(values, "id")))
        if action == "list_work":
            return [
                encoded(work)
                for work in await self.work_contexts(
                    namespace, subject, TypeAdapter(int).validate_python(values.pop("limit", 50))
                )
            ]
        if action == "work_history":
            return [
                encoded(work)
                for work in await self.work_context_history(
                    namespace,
                    subject,
                    required_text(values, "id"),
                    TypeAdapter(int).validate_python(values.pop("limit", 50)),
                )
            ]
        if action == "change_work":
            identity = required_text(values, "id")
            change = object_value(values.pop("change", {}))
            return {
                "result": encoded(
                    await self.change_work_context(
                        namespace,
                        subject,
                        ChangeWorkContext.model_validate(
                            {
                                "operation_id": operation_id,
                                "work_context_id": identity,
                                **values,
                                **change,
                            }
                        ),
                    )
                )
            }
        if action == "prepare":
            submitted = await self.submit_context_preparation(
                namespace,
                subject,
                ContextPrepare.model_validate({"operation_id": operation_id, **values}),
            )
            completed = (
                await self.wait_success(
                    (submitted.job_id,), namespace_id=namespace, subject_id=subject
                )
            )[0]
            output = object_value(completed.output)
            preparation_id = required_text(output, "preparation_id")
            return {"result": {"preparation_id": preparation_id}, "job_ids": [submitted.job_id]}
        if action in {"preparation", "cancel_preparation"}:
            identity = required_text(values, "id")
            result = (
                await self.context_preparation(namespace, subject, identity)
                if action == "preparation"
                else await self.cancel_context_preparation(
                    namespace, subject, identity, operation_id
                )
            )
            return encoded(result)
        if action == "admit":
            return {
                "result": encoded(
                    await self.admit_managed_context(
                        namespace,
                        subject,
                        ContextAdmissionCommand.model_validate(
                            {"operation_id": operation_id, **values}
                        ),
                    )
                )
            }
        if action == "admission":
            return encoded(
                await self.context_admission(namespace, subject, required_text(values, "id"))
            )
        if action == "report":
            return {
                "result": encoded(
                    await self.report_managed_context(
                        namespace,
                        subject,
                        ContextReport.model_validate({"operation_id": operation_id, **values}),
                    )
                )
            }
        raise ValueError("Unknown context action")
