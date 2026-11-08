"""Complete generation, template preview and model probes over shared content plans."""

from __future__ import annotations

import asyncio
from collections.abc import Callable, Sequence
from dataclasses import replace
from pathlib import Path
from typing import TYPE_CHECKING
from uuid import uuid4
from weakref import WeakValueDictionary

from pydantic import JsonValue

from nous.common.errors import NousError
from nous.common.operations import add_error_context, record_completed
from nous.content.contracts import (
    Artifact,
    ArtifactRef,
    InputSpec,
    OutputMapping,
    ProcessingResult,
    ResultRef,
    SelectedInputs,
    SourceSelection,
)
from nous.content.results import ResultStore
from nous.media.contracts import PreparedSource
from nous.models.configuration import ModelRoute
from nous.runtime.execution import ExecutionManager

from ..input import RequestGroup
from ..requests import FrozenTask, PreparedGeneration, UnavailableTask
from . import inspection
from .computation import (
    FrozenLanguageTask,
    freeze_task,
    load_task,
)
from .contracts import (
    GenerationOutcome,
    GenerationRequest,
    PendingResponse,
    PreparedProduction,
    ProducedTask,
)
from .preparation import GenerationPreparation
from .response import ResponseRecovery
from .transcription import TranscriptionOperations

if TYPE_CHECKING:
    from nous.configuration import ConfigSnapshot
    from nous.media.preparation import MediaPreparer
    from nous.storage.database import WriteTx

    from ..executor import ModelExecutor

InputValue = InputSpec | Path | Artifact | ArtifactRef | str


class GenerationOperations:
    """Coordinate language preparation, execution and result adoption.

    Only the active bounded request is materialized. Use ``produce`` when a
    caller must combine a completed model output with its own adoption. Dependencies
    are borrowed from the runtime.
    """

    def __init__(
        self,
        executor: ModelExecutor,
        results: ResultStore,
        execution: ExecutionManager,
        preparer: MediaPreparer,
    ) -> None:
        self.executor, self.results, self.execution, self.preparer = (
            executor,
            results,
            execution,
            preparer,
        )
        self._computation_locks: WeakValueDictionary[str, asyncio.Lock] = WeakValueDictionary()
        self._result_locks: WeakValueDictionary[str, asyncio.Lock] = WeakValueDictionary()
        self.responses = ResponseRecovery(executor, results, execution, self._computation_locks)
        self.transcription = TranscriptionOperations(
            executor, results, execution, self._computation_locks
        )
        self.preparation = GenerationPreparation(
            executor, results, execution, preparer, self.transcription
        )
        self.accept_inputs = self.preparation.accept_inputs
        self.prepare_input = self.preparation.prepare_input

    async def freeze_language_task(
        self,
        task: str | FrozenTask,
        snapshot: ConfigSnapshot,
        *,
        requires_transcription: bool = False,
        inputs: SelectedInputs | None = None,
        without_audio: bool = False,
    ) -> FrozenLanguageTask:
        frozen = self.executor.freeze(task, snapshot) if isinstance(task, str) else task
        if inputs is not None:
            for fact in inputs.facts:
                if fact.kind == "audio" and any(
                    not any(fmt.startswith("audio/") for fmt in route.model.input_formats)
                    for route in frozen.routes
                ):
                    requires_transcription = True
                if (
                    fact.kind == "video"
                    and not without_audio
                    and fact.details.get("has_audio")
                    and any(
                        not any(
                            fmt.startswith(("video/", "audio/"))
                            for fmt in route.model.input_formats
                        )
                        for route in frozen.routes
                    )
                ):
                    requires_transcription = True
        contract = (
            self.executor.compiler.catalog.tasks[
                frozen.resource.manifest.contract_id
            ].execution_contract()
            if frozen.resource
            else {}
        )
        transcription = frozen.transcription
        transcription_task = snapshot.values.media.audio.transcription_task
        if (
            requires_transcription
            and transcription_task
            and transcription_task != frozen.task
            and frozen.assignment.operation != "transcription"
            and transcription is None
        ):
            try:
                prerequisite = self.executor.freeze(transcription_task, snapshot)
            except NousError as error:
                transcription = UnavailableTask(
                    task=transcription_task,
                    code=error.code,
                    message=error.message,
                    details=error.details,
                    retryable=error.retryable,
                )
            else:
                transcription_contract = (
                    self.executor.compiler.catalog.tasks[
                        prerequisite.resource.manifest.contract_id
                    ].execution_contract()
                    if prerequisite.resource
                    else {}
                )
                transcription = (
                    await freeze_task(self.results, prerequisite, snapshot, transcription_contract)
                ).descriptor
        return await freeze_task(self.results, frozen, snapshot, contract, transcription)

    async def load_language_task(
        self, ref: ArtifactRef | FrozenLanguageTask, snapshot: ConfigSnapshot
    ) -> FrozenTask:
        return await load_task(
            self.results,
            ref.descriptor if isinstance(ref, FrozenLanguageTask) else ref,
            snapshot,
            self.executor.compiler.catalog.tasks,
        )

    def capabilities(self, snapshot: ConfigSnapshot) -> dict[str, JsonValue]:
        tasks: dict[str, JsonValue] = {}
        for name in snapshot.values.tasks:
            try:
                frozen = self.executor.freeze(name, snapshot)
                tasks[name] = {
                    "ready": True,
                    "prompt": frozen.resource.id if frozen.resource else None,
                    "routes": [route.model_id for route in frozen.routes],
                }
            except NousError as error:
                tasks[name] = {"ready": False, "error": error.to_dict()}
        return {"tasks": tasks}

    def require_contract(self, task: str, contract: str, snapshot: ConfigSnapshot) -> None:
        frozen = self.executor.freeze(task, snapshot)
        if frozen.resource is None or frozen.resource.manifest.contract_id != contract:
            raise NousError(
                "model.task_contract_mismatch",
                "Task uses another output contract",
                details={"task": task, "expected_contract": contract},
            )

    async def produce(
        self,
        request: GenerationRequest,
        *,
        computation_key: str,
        snapshot: ConfigSnapshot,
        on_delta: Callable[[str], None] | None = None,
    ) -> ProducedTask:
        """Restore an accepted response or perform one semantic language computation."""
        request = request.capture()
        lock = self._computation_locks.setdefault(computation_key, asyncio.Lock())
        async with self.execution.keyed_operation("model.produce", lock, snapshot):
            identity = request.identity()
            accepted = await self.results.find_producer(computation_key + ":response")
            if accepted is not None:
                if accepted.parameters.get("request_identity") != identity:
                    raise NousError(
                        "content.producer_conflict",
                        "Saved response belongs to a different language request",
                        details={"producer_key": computation_key},
                    )
                await self.responses.forget(computation_key + ":response")
                return await self.recover_response(accepted.ref, snapshot)
            pending = await self.responses.pending_for(computation_key + ":response")
            if pending is not None:
                if pending.identity != identity:
                    raise NousError(
                        "content.producer_conflict",
                        "Saved response belongs to a different language request",
                        details={"producer_key": computation_key},
                    )
                return await self.responses.resume_in_lock(pending)
            frozen = (
                await self.load_language_task(request.task, snapshot)
                if isinstance(request.task, FrozenLanguageTask)
                else self.executor.freeze(request.task, snapshot)
            )
            prepared = await self.prepare_input(
                frozen,
                request.inputs,
                snapshot,
                document_mode=request.document_mode,
                without_audio=request.without_audio,
                per_batch=request.mode == "independent_batches",
            )
            if len(prepared.input_plan.request_groups) != 1:
                raise NousError(
                    "model.single_computation_required",
                    "This computation requires one semantic request group",
                )
            selected_inputs = prepared.input_plan.selected_inputs
            task_ref = (
                request.task
                if isinstance(request.task, FrozenLanguageTask)
                else await self.freeze_language_task(
                    frozen, snapshot, inputs=selected_inputs, without_audio=request.without_audio
                )
            )
            if isinstance(request.task, str):
                retained = await self.load_language_task(task_ref, snapshot)
                prepared = replace(
                    prepared,
                    frozen=replace(
                        prepared.frozen,
                        execution_limits=retained.execution_limits,
                        media_settings=retained.media_settings,
                        transcription=retained.transcription,
                    ),
                )
            context_sources = tuple(
                [
                    PreparedSource(
                        key=group.key,
                        name=group.name,
                        text="\n".join(
                            [
                                await self.results.artifacts.read_selection(part.content)
                                for part in group.parts
                            ]
                        ),
                        group=group,
                        metadata={"representation": group.parts[0].representation},
                    )
                    for group in request.context
                ]
            )
            return await self._produce_group(
                prepared.frozen,
                prepared.input_plan.request_groups[0],
                snapshot,
                question=request.question,
                instructions=request.instructions,
                schema=request.schema,
                completed=list(prepared.preparations),
                on_delta=on_delta,
                producer_key=computation_key,
                frozen_task_ref=task_ref.descriptor,
                request_identity=identity,
                selected_inputs=selected_inputs,
                dependencies=(*request.dependencies, task_ref.descriptor),
                extra_sources=context_sources,
            )

    async def _produce_group(
        self,
        frozen: FrozenTask,
        group: RequestGroup,
        snapshot: ConfigSnapshot,
        *,
        question: str = "",
        instructions: str = "",
        schema: dict[str, JsonValue] | None = None,
        extra_sources: Sequence[PreparedSource] = (),
        completed: list[str] | None = None,
        on_delta: Callable[[str], None] | None = None,
        on_prepared: Callable[[PreparedGeneration], None] | None = None,
        producer_key: str | None = None,
        frozen_task_ref: ArtifactRef | None = None,
        request_identity: str | None = None,
        selected_inputs: SelectedInputs | None = None,
        dependencies: Sequence[ArtifactRef | ResultRef] = (),
    ) -> ProducedTask:
        """Compute one semantic group without adopting its named outputs.

        The executor requests another representation only when switching routes.
        The returned ``ProducedTask`` retains the successful output and its actual
        inputs; prepare and adopt them in the caller-owned transaction without
        making another model request. Completed preparation stages may already
        have been adopted before this method returns or fails.

        Returns:
            The successful output, actual inputs and completed preparation
            references, ready for adoption.

        Raises:
            NousError: If no eligible route can prepare or complete the request.
        """
        completed = completed if completed is not None else []
        async with self.execution.operation("model.generate", snapshot) as context:
            selected_inputs = selected_inputs or group.selected_inputs
            producer_key = producer_key or "generation:" + str(uuid4())

            async def resolve(route: ModelRoute | None = None) -> PreparedGeneration:
                request = await self.preparation.prepare_group(
                    frozen,
                    group,
                    snapshot,
                    question=question,
                    instructions=instructions,
                    schema=schema,
                    extra_sources=extra_sources,
                    route=route,
                    completed=completed,
                    on_prepared=on_prepared,
                    streaming=on_delta is not None,
                )
                assert isinstance(request, PreparedGeneration)
                return request

            prepared = await resolve()
            if on_delta is None:
                generated = await self.executor.execute(
                    prepared, context=context, prepare_route=resolve
                )
            else:
                async with self.executor.stream(
                    prepared, context=context, prepare_route=resolve
                ) as stream:
                    async for delta in stream:
                        on_delta(delta)
                    if stream.result is None:
                        raise NousError(
                            "model.incomplete_stream", "The stream returned no completed result"
                        )
                    generated = stream.result
            reading = generated.reading or prepared.compiled.reading
            response_key = producer_key + ":response"
            pending = PendingResponse(
                generated,
                None,
                reading,
                tuple(source.group for source in reading.sources if source.group),
                request_identity or producer_key,
                response_key,
                frozen_task_ref,
                selected_inputs,
                tuple(dependencies),
                self.executor.compiler.catalog.tasks[
                    str(generated.request["contract_id"])
                ].interpretation_contract(),
            )
            self.responses.pending[response_key] = pending
            return await self.responses.resume_in_lock(pending)

    async def run_input(
        self,
        task: str | FrozenTask,
        input: InputValue | Sequence[InputValue],
        snapshot: ConfigSnapshot,
        *,
        question: str = "",
        instructions: str = "",
        schema: dict[str, JsonValue] | None = None,
        document_mode: str = "auto",
        without_audio: bool = False,
        on_delta: Callable[[str], None] | None = None,
        on_prepared: Callable[[PreparedGeneration], None] | None = None,
        on_completed: Callable[[ProcessingResult], None] | None = None,
        texts: Sequence[str] = (),
        per_batch: bool = False,
    ) -> GenerationOutcome:
        results: list[ProcessingResult] = []
        completed: list[str] = []
        async with self.execution.operation("model.run", snapshot):
            prepared = await self.prepare_input(
                task,
                input,
                snapshot,
                document_mode=document_mode,
                without_audio=without_audio,
                texts=texts,
                per_batch=per_batch,
            )
            completed.extend(prepared.preparations)
            try:
                for plan in prepared.input_plan.request_groups:
                    produced = await self._produce_group(
                        prepared.frozen,
                        plan,
                        snapshot,
                        question=question,
                        instructions=instructions,
                        schema=schema,
                        completed=completed,
                        on_delta=on_delta,
                        on_prepared=on_prepared,
                    )
                    response_key = produced.parameters.get("response_producer_key")
                    assert isinstance(response_key, str) and response_key.endswith(":response")
                    result = await self._adopt(
                        produced, producer_key=response_key.removesuffix(":response") + ":result"
                    )
                    results.append(result)
                    completed.append(result.ref.identifier)
                    if on_completed:
                        on_completed(result)
            except (NousError, asyncio.CancelledError, TimeoutError) as exc:
                details: dict[str, JsonValue] = {
                    "artifacts": [artifact.ref.identifier for artifact in prepared.artifacts],
                    "completed_result_refs": list[JsonValue](completed),
                    "unfinished_coverage": [
                        batch.coverage
                        for batch in prepared.input_plan.request_groups[len(results) :]
                    ],
                }
                record_completed(completed)
                add_error_context(exc, details)
                raise
            return GenerationOutcome(
                prepared.artifacts,
                tuple(results),
                {
                    **prepared.input_plan.requested_coverage,
                    "actual": [result.coverage for result in results],
                },
                tuple(completed),
            )

    async def run(
        self,
        request: GenerationRequest,
        snapshot: ConfigSnapshot,
        *,
        computation_key: str | None = None,
    ) -> ProcessingResult:
        request = request.capture()
        key = computation_key or "generation:" + str(uuid4())
        lock = self._result_locks.setdefault(key, asyncio.Lock())
        async with self.execution.keyed_operation("model.run", lock, snapshot):
            accepted = await self.results.find_producer(key + ":result")
            if accepted is not None:
                if accepted.parameters.get("request_identity") != request.identity():
                    raise NousError(
                        "content.producer_conflict",
                        "Saved result belongs to a different language request",
                        details={"producer_key": key},
                    )
                return accepted
            produced = await self.produce(request, computation_key=key, snapshot=snapshot)
            return await self._adopt(produced, producer_key=key + ":result")

    async def prepare_adoption(
        self, produced: ProducedTask, *, producer_key: str | None = None
    ) -> PreparedProduction:
        """Publish the computation once before its short adoption transaction."""
        origins = tuple(
            dict.fromkeys(part.content for group in produced.actual_inputs for part in group.parts)
        )
        output = produced.outputs.get("output")
        mappings = (
            (
                OutputMapping(
                    output_name="output",
                    selection=SourceSelection(start_byte=0, end_byte=len(output.data)),
                    relation="derived",
                    origins=origins,
                ),
            )
            if output is not None and output.data and origins
            else ()
        )
        return PreparedProduction(
            produced.operation,
            produced.producer,
            produced.actual_inputs,
            await self.results.prepare_outputs(produced.outputs),
            produced.request,
            produced.parameters,
            produced.coverage,
            producer_key,
            produced.dependencies,
            mappings,
        )

    async def adopt_in(self, tx: WriteTx, prepared: PreparedProduction) -> ProcessingResult:
        """Adopt published output alongside the caller's related formal records."""
        return await self.results.adopt_in(
            tx,
            operation=prepared.operation,
            producer=prepared.producer,
            inputs=prepared.actual_inputs,
            prepared_outputs=prepared.outputs,
            request=prepared.request,
            parameters=prepared.parameters,
            coverage=prepared.coverage,
            producer_key=prepared.producer_key,
            dependencies=prepared.dependencies,
            mappings=prepared.mappings,
        )

    async def _adopt(
        self, produced: ProducedTask, *, producer_key: str | None = None
    ) -> ProcessingResult:
        prepared = None
        try:
            prepared = await self.prepare_adoption(produced, producer_key=producer_key)
            async with self.results.database.write() as tx:
                result = await self.adopt_in(tx, prepared)
            record_completed(result.ref.identifier)
            return result
        except BaseException as error:
            add_error_context(
                error,
                {
                    "stage": "result.adopt",
                    "model_succeeded": True,
                    "producer_key": produced.parameters.get("response_producer_key"),
                    "completed_result_refs": list(produced.completed_stage_refs),
                    "published_outputs": [
                        {"name": output.name, "blob": output.content.blob.model_dump(mode="json")}
                        for output in prepared.outputs.outputs
                    ]
                    if prepared is not None
                    else [],
                },
                recovery=prepared or produced,
            )
            raise

    async def resume_response(
        self, pending: PendingResponse | str, snapshot: ConfigSnapshot
    ) -> ProducedTask:
        """Resume a successful response by its in-memory value or saved producer key."""
        return await self.responses.resume_response(pending, snapshot)

    async def recover_result(self, response_key: str, snapshot: ConfigSnapshot) -> ProcessingResult:
        """Resume and adopt one successful response, returning the same result on retry."""
        if not response_key.endswith(":response"):
            raise NousError("model.response_required", "Provide the response producer key")
        key = response_key.removesuffix(":response")
        result_key = key + ":result"
        lock = self._result_locks.setdefault(key, asyncio.Lock())
        async with self.execution.keyed_operation("model.recover_result", lock, snapshot):
            accepted = await self.results.find_producer(result_key)
            if accepted is not None:
                await self.responses.forget(response_key)
                return accepted
            produced = await self.resume_response(response_key, snapshot)
            return await self._adopt(produced, producer_key=result_key)

    async def recover_response(
        self, ref: str | ResultRef, snapshot: ConfigSnapshot
    ) -> ProducedTask:
        """Interpret a previously accepted response without calling the model again."""
        return await self.responses.recover_response(ref, snapshot)

    async def preview(
        self,
        prompt: str,
        input: Sequence[InputValue],
        snapshot: ConfigSnapshot,
        *,
        texts: Sequence[str] = (),
        question: str = "",
        instructions: str = "",
        schema: dict[str, JsonValue] | None = None,
        task: str | None = None,
        per_batch: bool = False,
    ) -> dict[str, JsonValue]:
        return await inspection.preview(
            self,
            prompt,
            input,
            snapshot,
            texts=texts,
            question=question,
            instructions=instructions,
            schema=schema,
            task=task,
            per_batch=per_batch,
        )

    async def probe(
        self, model: str, input: Path | None, snapshot: ConfigSnapshot
    ) -> GenerationOutcome:
        return await inspection.probe(self, model, input, snapshot)
