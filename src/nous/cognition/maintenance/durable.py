"""Register the two cognitive workflows only for an explicitly owned host."""

from __future__ import annotations

from collections.abc import Generator
from contextlib import contextmanager
from typing import TYPE_CHECKING, cast

import structlog
from dbos import DBOS, DBOSConfiguredInstance, WorkflowSerializationFormat
from opentelemetry import context as otel_context
from opentelemetry.propagate import extract
from pydantic import JsonValue

from nous.common.errors import NousError
from nous.content.contracts import ResultRef

from ..annotation.contracts import AnnotationPlan, AnnotationRequest
from ..contracts import NamespaceRef, RecordRevisionRef
from .contracts import RefreshRequest
from .definitions import ANNOTATION, CLASS_NAME, REFRESH

if TYPE_CHECKING:
    from nous.runtime.execution import ExecutionManager

    from .operations import MaintenanceOperations

JsonObject = dict[str, JsonValue]


def register(
    instance_id: str, execution: ExecutionManager, maintenance: MaintenanceOperations
) -> DBOSConfiguredInstance:
    """Bind fully constructed business services to this host's workflow registry."""

    @contextmanager
    def association(payload: JsonObject, step: str) -> Generator[None]:
        trace = payload.get("trace")
        carrier = (
            {
                key: str(value)
                for key, value in trace.items()
                if key in {"traceparent", "tracestate"}
            }
            if isinstance(trace, dict)
            else {}
        )
        token = otel_context.attach(extract(carrier))
        try:
            with structlog.contextvars.bound_contextvars(
                job_id=str(DBOS.workflow_id or payload.get("job_id", "")), job_step=step
            ):
                yield
        finally:
            otel_context.detach(token)

    @DBOS.dbos_class(CLASS_NAME)
    class NousMaintenance(DBOSConfiguredInstance):
        @DBOS.step(name="nous.annotation.prepare", preemptible=True)
        async def prepare(self, payload: JsonObject) -> JsonObject:
            with association(payload, "prepare"):
                try:
                    plan = await maintenance.prepare_annotation(
                        RecordRevisionRef.model_validate(payload["target_revision"]),
                        AnnotationRequest.model_validate(payload["request"]),
                    )
                except NousError as error:
                    if error.code != "cognition.annotation_conflict":
                        raise
                    return {"state": "superseded"}
                return plan.model_dump(mode="json")

        @DBOS.step(name="nous.annotation.generate", preemptible=True)
        async def generate(self, value: JsonObject) -> JsonObject:
            with association(value, "generate"):
                result = await maintenance.generate_annotation(
                    AnnotationPlan.model_validate(value["plan"])
                )
                return result.model_dump(mode="json")

        @DBOS.step(name="nous.annotation.adopt", preemptible=True)
        async def adopt(self, value: JsonObject) -> JsonObject:
            with association(value, "adopt"):
                refresh = value.get("refresh")
                result = await maintenance.adopt_annotation(
                    AnnotationPlan.model_validate(value["plan"]),
                    ResultRef.model_validate(value["proposal"]),
                    RefreshRequest.model_validate(refresh) if refresh is not None else None,
                )
                return result.model_dump(mode="json")

        @DBOS.step(name="nous.refresh.build", preemptible=True)
        async def build(self, payload: JsonObject) -> JsonObject:
            with association(payload, "refresh"):
                result = await maintenance.refresh(
                    NamespaceRef.model_validate(payload["namespace_ref"]),
                    int(cast(int, payload["minimum_generation"])),
                    RefreshRequest.model_validate(payload["request"]),
                    build_key=str(payload["job_id"]),
                )
                return result.model_dump(mode="json")

        @DBOS.workflow(
            name=ANNOTATION,
            max_recovery_attempts=20,
            serialization_type=WorkflowSerializationFormat.PORTABLE,
        )
        async def annotation(self, payload: JsonObject) -> JsonObject:
            async with execution.durable():
                plan = await self.prepare(payload)
                if plan.get("state") == "superseded":
                    return plan
                proposal = await self.generate(
                    {
                        "plan": plan,
                        "trace": payload.get("trace", {}),
                    }
                )
                return await self.adopt(
                    {
                        "plan": plan,
                        "proposal": proposal,
                        "refresh": payload.get("refresh"),
                        "trace": payload.get("trace", {}),
                    }
                )

        @DBOS.workflow(
            name=REFRESH,
            max_recovery_attempts=20,
            serialization_type=WorkflowSerializationFormat.PORTABLE,
        )
        async def refresh(self, payload: JsonObject) -> JsonObject:
            async with execution.durable():
                return await self.build(payload)

    return NousMaintenance(instance_id)
