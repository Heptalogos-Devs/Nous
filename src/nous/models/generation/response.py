"""Publish, adopt and interpret successful language responses."""

from __future__ import annotations

import asyncio
import json
from collections.abc import Mapping, Sequence
from dataclasses import replace
from typing import TYPE_CHECKING, cast
from weakref import WeakValueDictionary

from pydantic import JsonValue, TypeAdapter

from nous.common.errors import NousError
from nous.common.operations import add_error_context, record_completed
from nous.content.contracts import (
    InputGroup,
    OutputContent,
    ProcessingResult,
    ResultRef,
    SelectedInputs,
)
from nous.content.results import ResultStore
from nous.observability.instrumentation import get_logger
from nous.runtime.execution import ExecutionManager

from ..contracts import GenerationResult
from .computation import descriptor_text, generated_value, read_generated
from .contracts import PendingResponse, ProducedTask
from .response_journal import ResponseJournal

if TYPE_CHECKING:
    from nous.configuration import ConfigSnapshot

    from ..executor import ModelExecutor


class ResponseRecovery:
    """Retain successful responses and adopt or interpret them without re-execution."""

    def __init__(
        self,
        executor: ModelExecutor,
        results: ResultStore,
        execution: ExecutionManager,
        locks: WeakValueDictionary[str, asyncio.Lock],
    ) -> None:
        self.executor, self.results, self.execution, self.locks = (
            executor,
            results,
            execution,
            locks,
        )
        self.pending: dict[str, PendingResponse] = {}
        self.journal = ResponseJournal(results.artifacts.content.root, execution)

    async def pending_for(self, key: str) -> PendingResponse | None:
        pending = self.pending.get(key)
        if pending is None:
            pending = await self.journal.load(key)
            if pending is not None:
                self.pending[key] = pending
        return pending

    async def forget(self, key: str) -> None:
        self.pending.pop(key, None)
        try:
            await self.journal.discard(key)
        except OSError as error:
            get_logger(__name__).warning(
                "model.response_recovery_cleanup_failed",
                extra={"error_type": type(error).__name__},
            )

    async def _publish_response(self, pending: PendingResponse) -> PendingResponse:
        generated = pending.generated
        published = await self.results.prepare_outputs(
            {
                "response": OutputContent(
                    data=json.dumps(generated.response, ensure_ascii=False).encode(),
                    media_type="application/json",
                    display_name="response.json",
                ),
                "computation": OutputContent(
                    data=json.dumps(
                        generated_value(generated, pending.reading), ensure_ascii=False
                    ).encode(),
                    media_type="application/json",
                    display_name="response-computation.json",
                ),
            }
        )
        return replace(pending, outputs=published)

    async def resume_in_lock(
        self,
        pending: PendingResponse,
        *,
        inputs: Sequence[InputGroup] | None = None,
        coverage: Mapping[str, object] | None = None,
    ) -> ProducedTask:
        try:
            existing = self.pending.get(pending.producer_key)
            if (
                existing is not None
                and existing is not pending
                and not self.journal.same_response(existing, pending)
            ):
                raise NousError(
                    "content.producer_conflict",
                    "Another successful response is pending for this producer key",
                    details={"producer_key": pending.producer_key},
                )
            self.pending.setdefault(pending.producer_key, pending)
            await self.journal.save(pending)
            if pending.outputs is None:
                pending = await self._publish_response(pending)
                self.pending[pending.producer_key] = pending
            accepted = await self._adopt_response(pending)
        except BaseException as error:
            add_error_context(
                error,
                {
                    "stage": "model.response_adopt",
                    "model_succeeded": True,
                    "producer_key": pending.producer_key,
                },
                recovery=pending,
            )
            raise
        await self.forget(pending.producer_key)
        return await self._interpret_response(
            pending.generated,
            accepted,
            pending.producer_key,
            inputs if inputs is not None else pending.requested_inputs,
            coverage if coverage is not None else pending.requested_coverage,
        )

    async def _adopt_response(self, pending: PendingResponse) -> ProcessingResult:
        assert pending.outputs is not None
        async with self.results.database.write() as tx:
            result = await self.results.adopt_in(
                tx,
                operation="model.language_response",
                producer=pending.generated.producer,
                inputs=pending.inputs,
                prepared_outputs=pending.outputs,
                request=pending.generated.request,
                parameters={
                    "request_identity": pending.identity,
                    "producer_key": pending.producer_key,
                    "frozen_task_ref": pending.frozen_task_ref.model_dump(mode="json")
                    if pending.frozen_task_ref
                    else None,
                    "selected_inputs": pending.selected_inputs.model_dump(mode="json")
                    if pending.selected_inputs
                    else None,
                    "interpretation_contract": pending.interpretation_contract,
                    "requested_inputs": [
                        group.model_dump(mode="json") for group in pending.requested_inputs
                    ]
                    if pending.requested_inputs is not None
                    else None,
                    "requested_coverage": pending.requested_coverage,
                },
                coverage=pending.generated.coverage,
                producer_key=pending.producer_key,
                dependencies=pending.dependencies,
            )
        record_completed(result.ref.identifier)
        return result

    async def resume_response(
        self, pending: PendingResponse | str, snapshot: ConfigSnapshot
    ) -> ProducedTask:
        """Resume a successful response by its in-memory value or saved producer key."""
        response_key = pending if isinstance(pending, str) else pending.producer_key
        key = response_key.removesuffix(":response")
        lock = self.locks.setdefault(key, asyncio.Lock())
        async with self.execution.keyed_operation("model.resume_response", lock, snapshot):
            accepted = await self.results.find_producer(response_key)
            if accepted is not None:
                if isinstance(pending, PendingResponse) and (
                    accepted.parameters.get("request_identity") != pending.identity
                    or accepted.parameters.get("requested_inputs")
                    != (
                        [group.model_dump(mode="json") for group in pending.requested_inputs]
                        if pending.requested_inputs is not None
                        else None
                    )
                    or accepted.parameters.get("requested_coverage") != pending.requested_coverage
                ):
                    raise NousError(
                        "content.producer_conflict",
                        "Saved response belongs to a different language request",
                        details={"producer_key": response_key},
                    )
                await self.forget(response_key)
                return await self.recover_response(accepted.ref, snapshot)
            if isinstance(pending, str):
                restored = await self.pending_for(response_key)
                if restored is None:
                    raise NousError(
                        "model.response_not_found",
                        "No recoverable response exists for this producer key",
                        details={"producer_key": response_key},
                    )
            else:
                restored = pending
            return await self.resume_in_lock(restored)

    async def recover_response(
        self,
        ref: str | ResultRef,
        snapshot: ConfigSnapshot,
    ) -> ProducedTask:
        """Interpret an accepted response using its saved inputs and installed task contract."""
        async with self.execution.operation("model.recover_response", snapshot):
            accepted = await self.results.get(ref)
            if accepted.operation != "model.language_response":
                raise NousError(
                    "model.response_required", "Recovery requires an accepted language response"
                )
            envelope = next(
                output.artifact for output in accepted.outputs if output.name == "computation"
            )
            generated = read_generated(json.loads(await descriptor_text(self.results, envelope)))
            requested_inputs = accepted.parameters.get("requested_inputs")
            requested_coverage = accepted.parameters.get("requested_coverage")
            return await self._interpret_response(
                generated,
                accepted,
                str(accepted.parameters.get("producer_key") or accepted.ref.identifier),
                TypeAdapter(tuple[InputGroup, ...]).validate_python(requested_inputs)
                if requested_inputs is not None
                else None,
                TypeAdapter(dict[str, JsonValue]).validate_python(requested_coverage)
                if requested_coverage is not None
                else None,
            )

    def _validate_interpreter(self, generated: GenerationResult, expected: object) -> None:
        contract_id = str(generated.request.get("contract_id", ""))
        tasks = self.executor.compiler.catalog.tasks
        if contract_id not in tasks:
            raise NousError(
                "model.task_contract_changed",
                "The task interpreter for this response is not installed",
                details={"contract_id": contract_id},
            )
        definition = tasks[contract_id]
        if definition.interpretation_contract() != expected:
            raise NousError(
                "model.task_contract_changed",
                "Installed task interpreter differs from the saved response",
                details={"contract_id": contract_id},
            )
        if not definition.supports_schema(generated.request.get("schema")):
            raise NousError(
                "model.task_contract_changed",
                "Installed output schema differs from the accepted response",
            )

    async def _interpret_response(
        self,
        generated: GenerationResult,
        accepted: ProcessingResult,
        producer_key: str,
        inputs: Sequence[InputGroup] | None,
        coverage: Mapping[str, object] | None,
    ) -> ProducedTask:
        try:
            self._validate_interpreter(
                generated, accepted.parameters.get("interpretation_contract")
            )
            produced = await self._interpret(generated, inputs, coverage)
            return replace(
                produced,
                selected_inputs=SelectedInputs.model_validate(
                    accepted.parameters["selected_inputs"]
                )
                if accepted.parameters.get("selected_inputs")
                else None,
                dependencies=(*accepted.dependencies, accepted.ref),
                parameters={
                    **produced.parameters,
                    "selected_inputs": accepted.parameters.get("selected_inputs"),
                    "request_identity": accepted.parameters.get("request_identity"),
                    "response_producer_key": producer_key,
                },
                completed_stage_refs=tuple(
                    dict.fromkeys((*produced.completed_stage_refs, accepted.ref.identifier))
                ),
            )
        except BaseException as error:
            add_error_context(
                error,
                {
                    "stage": "model.interpret",
                    "model_succeeded": True,
                    "response_result_ref": accepted.ref.model_dump(mode="json"),
                    "producer_key": producer_key,
                    "completed_result_refs": [accepted.ref.identifier],
                },
            )
            raise

    async def _interpret(
        self,
        generated: GenerationResult,
        inputs: Sequence[InputGroup] | None,
        coverage: Mapping[str, object] | None,
    ) -> ProducedTask:
        reading = generated.reading
        if reading is None:
            raise NousError("model.response_invalid", "Saved response has no actual input context")
        definition = self.executor.compiler.catalog.tasks[str(generated.request["contract_id"])]
        outputs = await definition.outputs(
            generated.output,
            reading,
            self.results.artifacts,
            structured=generated.request.get("schema") is not None,
            producer=generated.producer,
        )
        if generated.response:
            outputs["response"] = OutputContent(
                data=json.dumps(generated.response, ensure_ascii=False, indent=2).encode(),
                media_type="application/json",
                display_name="response.json",
            )
        stages = tuple(cast(list[str], generated.request.get("stage_result_refs", [])))
        return ProducedTask(
            operation=str(generated.request["task"]),
            producer=generated.producer,
            actual_inputs=tuple(inputs)
            if inputs is not None
            else tuple(source.group for source in reading.sources if source.group),
            outputs=outputs,
            request=generated.request,
            parameters={
                "attempts": generated.attempts,
                "program_outputs": list(definition.program_outputs),
                "representations": {"output": {"kind": definition.source_representation}}
                if definition.source_representation is not None
                else {},
                "usage": generated.usage,
                "finish_reason": generated.finish_reason,
                "stage_result_refs": generated.request.get("stage_result_refs", []),
            },
            coverage=generated.coverage
            or TypeAdapter(dict[str, JsonValue]).validate_python(coverage or {}),
            completed_stage_refs=stages,
        )
