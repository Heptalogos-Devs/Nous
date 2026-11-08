"""Durable representations of successful language responses awaiting adoption."""

from __future__ import annotations

from pathlib import Path
from typing import Literal

from pydantic import BaseModel, ConfigDict, JsonValue, ValidationError

from nous.content.contracts import ArtifactRef, InputGroup, ResultRef, SelectedInputs
from nous.runtime.execution import ExecutionManager

from .computation import generated_value, read_generated
from .contracts import PendingResponse
from .journal import RecoveryFiles


class _Dependency(BaseModel):
    model_config = ConfigDict(extra="forbid")

    kind: Literal["artifact", "result"]
    ref: dict[str, str]


class _SavedResponse(BaseModel):
    model_config = ConfigDict(extra="forbid")

    producer_key: str
    identity: str
    generated: dict[str, JsonValue]
    inputs: tuple[InputGroup, ...]
    frozen_task_ref: ArtifactRef | None
    selected_inputs: SelectedInputs | None
    dependencies: tuple[_Dependency, ...]
    interpretation_contract: dict[str, JsonValue]

    @classmethod
    def capture(cls, pending: PendingResponse) -> _SavedResponse:
        return cls(
            producer_key=pending.producer_key,
            identity=pending.identity,
            generated=generated_value(pending.generated, pending.reading),
            inputs=pending.inputs,
            frozen_task_ref=pending.frozen_task_ref,
            selected_inputs=pending.selected_inputs,
            dependencies=tuple(
                _Dependency(
                    kind="artifact" if isinstance(ref, ArtifactRef) else "result",
                    ref=ref.model_dump(mode="json"),
                )
                for ref in pending.dependencies
            ),
            interpretation_contract=pending.interpretation_contract,
        )

    def pending(self) -> PendingResponse:
        generated = read_generated(self.generated)
        if generated.reading is None:
            raise ValueError("saved response has no reading input")
        dependencies: tuple[ArtifactRef | ResultRef, ...] = tuple(
            ArtifactRef.model_validate(item.ref)
            if item.kind == "artifact"
            else ResultRef.model_validate(item.ref)
            for item in self.dependencies
        )
        return PendingResponse(
            generated=generated,
            outputs=None,
            reading=generated.reading,
            inputs=self.inputs,
            identity=self.identity,
            producer_key=self.producer_key,
            frozen_task_ref=self.frozen_task_ref,
            selected_inputs=self.selected_inputs,
            dependencies=dependencies,
            interpretation_contract=self.interpretation_contract,
        )


class ResponseJournal:
    """Preserve a successful language computation before publishing its bytes."""

    def __init__(self, content_root: Path, execution: ExecutionManager) -> None:
        self.files = RecoveryFiles(
            content_root / "pending" / "responses",
            execution,
            invalid_code="model.response_recovery_invalid",
            conflict_code="model.response_recovery_conflict",
        )

    @staticmethod
    def same_response(left: PendingResponse, right: PendingResponse) -> bool:
        """Compare the complete durable semantics, excluding transient media buffers."""
        return _SavedResponse.capture(left) == _SavedResponse.capture(right)

    async def load(self, key: str) -> PendingResponse | None:
        raw = await self.files.load(key)
        if raw is None:
            return None
        try:
            return _SavedResponse.model_validate(raw).pending()
        except (ValidationError, ValueError, KeyError, TypeError) as error:
            raise self.files.invalid(error) from error

    async def save(self, pending: PendingResponse) -> None:
        value = _SavedResponse.capture(pending)
        await self.files.save(pending.producer_key, value.model_dump(mode="json"))

    async def discard(self, key: str) -> None:
        await self.files.discard(key)
