"""The two stable DBOS workflow contracts for cognitive maintenance."""

from typing import Protocol

from dbos import DBOS, DBOSConfiguredInstance, WorkflowSerializationFormat

from .bridge import JsonObject, RuntimeBridge

CONTRACT_VERSION = "nous-durable-cognition-1"
RECOVERY_ATTEMPTS = 20


class MaintenanceStages(Protocol):
    async def prepare_annotation(self, payload: JsonObject) -> JsonObject: ...
    async def generate_annotation(self, plan: JsonObject) -> JsonObject: ...
    async def adopt_annotation(self, value: JsonObject) -> JsonObject: ...
    async def refresh(self, payload: JsonObject) -> JsonObject: ...


@DBOS.dbos_class()
class NousMaintenance(DBOSConfiguredInstance):
    """Registered once for an instance; the current host binds its own stages."""

    def __init__(self, instance_id: str, bridge: RuntimeBridge, stages: MaintenanceStages) -> None:
        super().__init__(instance_id)
        self.bridge: RuntimeBridge | None = bridge
        self.stages: MaintenanceStages | None = stages

    def bind(self, bridge: RuntimeBridge, stages: MaintenanceStages) -> None:
        self.bridge = bridge
        self.stages = stages

    def unbind(self) -> None:
        self.bridge = None
        self.stages = None

    def bound(self) -> tuple[RuntimeBridge, MaintenanceStages]:
        if self.bridge is None or self.stages is None:
            raise RuntimeError("Maintenance host has stopped")
        return self.bridge, self.stages

    def association(self, payload: JsonObject, step: str) -> dict[str, str]:
        result = {"job_id": str(DBOS.workflow_id or payload.get("job_id", "")), "step": step}
        plan = payload.get("plan")
        carrier = (plan if isinstance(plan, dict) else payload).get("trace")
        if isinstance(carrier, dict):
            for key in ("traceparent", "tracestate"):
                value = carrier.get(key)
                if isinstance(value, str):
                    result[key] = value
        return result

    @DBOS.step(name="nous.annotation.prepare", preemptible=True)
    async def prepare_annotation(self, payload: JsonObject) -> JsonObject:
        bridge, stages = self.bound()
        return await bridge.call(
            stages.prepare_annotation, payload, association=self.association(payload, "prepare")
        )

    @DBOS.step(name="nous.annotation.generate", preemptible=True)
    async def generate_annotation(self, plan: JsonObject) -> JsonObject:
        bridge, stages = self.bound()
        return await bridge.call(
            stages.generate_annotation, plan, association=self.association(plan, "generate")
        )

    @DBOS.step(name="nous.annotation.adopt", preemptible=True)
    async def adopt_annotation(self, value: JsonObject) -> JsonObject:
        bridge, stages = self.bound()
        return await bridge.call(
            stages.adopt_annotation, value, association=self.association(value, "adopt")
        )

    @DBOS.step(name="nous.refresh.build", preemptible=True)
    async def build_refresh(self, payload: JsonObject) -> JsonObject:
        bridge, stages = self.bound()
        return await bridge.call(
            stages.refresh, payload, association=self.association(payload, "refresh")
        )

    @DBOS.workflow(
        max_recovery_attempts=RECOVERY_ATTEMPTS,
        serialization_type=WorkflowSerializationFormat.PORTABLE,
    )
    async def annotation(self, payload: JsonObject) -> JsonObject:
        plan = await self.prepare_annotation(payload)
        if plan.get("state") in {"skipped", "superseded"}:
            return plan
        generated = await self.generate_annotation(plan)
        return await self.adopt_annotation({"plan": plan, "generated": generated})

    @DBOS.workflow(
        max_recovery_attempts=RECOVERY_ATTEMPTS,
        serialization_type=WorkflowSerializationFormat.PORTABLE,
    )
    async def refresh(self, payload: JsonObject) -> JsonObject:
        return await self.build_refresh(payload)


WORKFLOW_NAMES = {
    "annotation": f"{NousMaintenance.__qualname__}.annotation",
    "refresh": f"{NousMaintenance.__qualname__}.refresh",
}
CLASS_NAME = NousMaintenance.__qualname__
