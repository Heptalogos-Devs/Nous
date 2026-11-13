"""Execute language tasks and adopt completed results."""

from __future__ import annotations

import asyncio
import json
from typing import TYPE_CHECKING
from weakref import WeakValueDictionary

from pydantic import JsonValue, TypeAdapter

from nous.application.contracts import CallContext, LanguageEvents, TextDelta, define_operation
from nous.common.errors import NousError
from nous.common.operations import add_error_context
from nous.content.contracts import (
    ExternalInput,
    InlineText,
    ModelProducer,
    OutputContent,
    ProcessingResult,
    ResultFacts,
    ResultRef,
    SelectedInputs,
)
from nous.content.results import ResultStore
from nous.models.configuration import ModelRoute, PresetConfig, TaskAssignment
from nous.runtime.execution import ExecutionManager

from ..contracts import ReadingInput
from ..input import RoutePlan, SemanticUnit, materialize, plan_inputs
from ..requests import FrozenTask, PreparedGeneration
from .contracts import (
    CompletedTask,
    DiscoverModels,
    GenerationRequest,
    GenerationRequestFacts,
    ModelOutcome,
    PreviewPrompt,
    ProbeModel,
    RunModel,
)
from .preparation import GenerationPreparation
from .transcription import TranscriptionOperations

if TYPE_CHECKING:
    from nous.configuration import ConfigSnapshot
    from nous.media.preparation import MediaPreparer

    from ..executor import ModelExecutor


MODEL_RUN = define_operation(
    "models.run",
    request=RunModel,
    result=ModelOutcome,
    events=LanguageEvents,
    description="Run a model task on selected sources and save its results.",
)
PROMPT_PREVIEW = define_operation(
    "prompts.preview",
    request=PreviewPrompt,
    result=dict[str, JsonValue],
    events=LanguageEvents,
    description="Preview the prompt and selected input representations for a task.",
)
MODEL_PROBE = define_operation(
    "models.probe",
    request=ProbeModel,
    result=ModelOutcome,
    events=LanguageEvents,
    description="Test a configured model with a small request.",
)
MODEL_DISCOVER = define_operation(
    "models.discover",
    request=DiscoverModels,
    result=dict[str, JsonValue],
    events=LanguageEvents,
    description="Discover models offered by a configured provider.",
)


class GenerationOperations:
    """Interpret a language call once; the caller decides where to adopt its output."""

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
        self._transcription_locks: WeakValueDictionary[str, asyncio.Lock] = WeakValueDictionary()
        self.transcription = TranscriptionOperations(
            executor, results, execution, self._transcription_locks
        )
        self.preparation = GenerationPreparation(
            executor, results, execution, preparer, self.transcription
        )

    def capabilities(self, snapshot: ConfigSnapshot) -> dict[str, JsonValue]:
        tasks: dict[str, JsonValue] = {}
        for name in snapshot.values.tasks:
            try:
                task = self.executor.freeze(name, snapshot)
                tasks[name] = {
                    "ready": True,
                    "prompt": task.resource.id if task.resource else None,
                    "routes": [route.model_id for route in task.routes],
                }
            except NousError as error:
                tasks[name] = {"ready": False, "error": error.to_dict()}
        return {"tasks": tasks}

    def require_contract(self, task: str, contract: str, snapshot: ConfigSnapshot) -> None:
        resolved = self.executor.freeze(task, snapshot)
        if resolved.resource is None or resolved.resource.manifest.contract_id != contract:
            raise NousError(
                "model.task_contract_mismatch",
                "Task uses another output contract",
                details={"task": task, "expected_contract": contract},
            )

    async def resolve_schema(
        self, value: dict[str, JsonValue] | ExternalInput | None, call: CallContext
    ) -> dict[str, JsonValue] | None:
        """Read a caller-provided schema within this request's input deadline."""
        if isinstance(value, ExternalInput):
            selected = await call.inputs.read_text(
                value.key, value.selection, call.settings.values.content.text_read_max_bytes
            )
            return TypeAdapter(dict[str, JsonValue]).validate_json(selected.data)
        return value

    async def compute(self, request: GenerationRequest, call: CallContext) -> CompletedTask:
        """Complete one semantic task without persisting its output."""
        if (
            request.input_mode == "independent"
            and sum(
                len(source.selections)
                for source in request.inputs.sources
                if source.role != "context"
            )
            > 1
        ):
            raise NousError(
                "model.single_computation_required", "Compute requires one semantic unit"
            )
        prepared = await self.preparation.prepare_input(
            request.task,
            request.inputs,
            call.settings,
            document_mode=request.document_mode,
            without_audio=request.without_audio,
            input_mode=request.input_mode,
            unit=SemanticUnit("U1", request.inputs, request.input_mode),
        )
        if len(prepared.input_plan.route_plans) != 1:
            raise NousError(
                "model.single_computation_required", "Compute requires one semantic unit"
            )
        return await self._compute_group(
            request,
            prepared.frozen,
            prepared.input_plan.route_plans[0],
            call,
            list(prepared.preparations),
        )

    async def _compute_group(
        self,
        request: GenerationRequest,
        frozen: FrozenTask,
        group: RoutePlan,
        call: CallContext,
        completed: list[ResultRef],
    ) -> CompletedTask:
        async def resolve(route: ModelRoute | None = None) -> PreparedGeneration:
            value = await self.preparation.prepare_group(
                frozen,
                group,
                call.settings,
                question=request.question,
                instructions=request.instructions,
                schema=request.output_schema,
                route=route,
                completed=completed,
                streaming=request.stream,
            )
            assert isinstance(value, PreparedGeneration)
            return value

        prepared = await resolve()
        if request.stream:
            async with self.executor.stream(
                prepared, context=call.execution, prepare_route=resolve
            ) as stream:
                async for delta in stream:
                    await call.events.emit(TextDelta(channel="output", text=delta))
                if stream.result is None:
                    raise NousError(
                        "model.incomplete_stream", "The stream returned no completed result"
                    )
                generated = stream.result
        else:
            generated = await self.executor.execute(
                prepared, context=call.execution, prepare_route=resolve
            )
        reading = generated.reading or prepared.compiled.reading
        if frozen.resource is None:
            raise NousError("model.task_contract_missing", "The task has no output contract")
        definition = self.executor.compiler.catalog.tasks[frozen.resource.manifest.contract_id]
        interpreted = await definition.outputs(
            generated.output,
            reading,
            self.results.artifacts,
            structured=prepared.compiled.schema is not None,
            producer=generated.producer,
        )
        outputs = definition.describe_outputs(interpreted.outputs)
        if generated.response:
            outputs["response"] = OutputContent(
                data=json.dumps(generated.response, ensure_ascii=False, indent=2).encode(),
                media_type="application/json",
                display_name="response.json",
                role="diagnostic",
            )
        actual = tuple(source.group for source in reading.sources if source.group is not None)
        selected = group.unit.inputs
        artifacts = await self.results.artifacts.get_many(
            [item.artifact for source in selected.sources for item in source.selections]
        )
        dependencies = tuple(
            dict.fromkeys(
                [
                    artifact.producer_result_ref
                    for artifact in artifacts
                    if artifact.producer_result_ref
                ]
                + [
                    origin.transform_ref
                    for item in actual
                    for part in item.parts
                    for origin in part.origins
                    if origin.transform_ref
                ]
            )
        )
        request_fields = {
            field.alias or name for name, field in GenerationRequestFacts.model_fields.items()
        }
        recorded_request = {
            key: value for key, value in generated.request.items() if key in request_fields
        }
        recorded_request.update(question=request.question, instructions=request.instructions)
        request_facts = GenerationRequestFacts.model_validate(recorded_request)
        facts = ResultFacts(
            operation=request.task,
            producer=ModelProducer.model_validate(generated.producer),
            selected_inputs=selected,
            inputs=actual,
            request=request_facts.model_dump(mode="json", by_alias=True),
            parameters={
                "attempts": generated.attempts,
                "usage": generated.usage,
                "finish_reason": generated.finish_reason,
            },
            coverage=generated.coverage,
            mappings=interpreted.mappings,
            selections=interpreted.selections,
            dependencies=dependencies,
            formed_at=self.results.artifacts.clock.utc_now(),
        )
        return CompletedTask(facts=facts, outputs=outputs)

    async def run(self, request: RunModel, call: CallContext) -> ModelOutcome:
        output_schema = await self.resolve_schema(request.output_schema, call)
        selected = await self.preparation.accept_targets(request.sources, call)
        internal = GenerationRequest(
            task=request.task,
            inputs=selected,
            question=request.question,
            instructions=request.instructions,
            output_schema=output_schema,
            input_mode=request.input_mode,
            document_mode=request.document_mode,
            without_audio=request.without_audio,
            stream=request.stream,
        )
        planned = await self.preparation.prepare_input(
            request.task,
            selected,
            call.settings,
            document_mode=request.document_mode,
            without_audio=request.without_audio,
            input_mode=request.input_mode,
        )
        completed = list(planned.preparations)
        results: list[ProcessingResult] = []
        try:
            for group in planned.input_plan.route_plans:
                task = await self._compute_group(internal, planned.frozen, group, call, completed)
                result = await self.results.adopt(facts=task.facts, outputs=task.outputs)
                results.append(result)
                completed.append(result.ref)
                await call.saved(result.ref)
        except (NousError, asyncio.CancelledError, TimeoutError) as error:
            add_error_context(
                error,
                {
                    "completed_refs": [item.model_dump(mode="json") for item in completed],
                    "unfinished_coverage": [
                        group.coverage for group in planned.input_plan.route_plans[len(results) :]
                    ],
                },
            )
            raise
        return ModelOutcome(
            results=tuple(item.ref for item in results),
            coverage={
                **planned.input_plan.requested_coverage,
                "actual": [item.coverage for item in results],
            },
        )

    async def preview(self, request: PreviewPrompt, call: CallContext) -> dict[str, JsonValue]:
        output_schema = await self.resolve_schema(request.output_schema, call)
        resource = self.executor.compiler.catalog.get(request.prompt)
        frozen = self.executor.freeze(request.task, call.settings) if request.task else None
        if frozen and (frozen.resource is None or frozen.resource.id != resource.id):
            raise NousError("model.prompt_conflict", "Prompt does not match the selected task")
        selected = (
            await self.preparation.accept_targets(request.sources, call)
            if request.sources
            else SelectedInputs(sources=())
        )
        if frozen and selected.sources:
            prepared = await self.preparation.prepare_input(
                frozen, selected, call.settings, input_mode=request.input_mode
            )
            plan, frozen = prepared.input_plan, prepared.frozen
        else:
            plan = await plan_inputs(
                self.preparer,
                selected,
                call.settings.values.media,
                requirements=frozen.routes[0].model.input_formats if frozen else (),
                input_mode=request.input_mode,
                contract=self.executor.compiler.catalog.tasks[resource.manifest.contract_id],
            )
        previews: list[JsonValue] = []
        for group in plan.route_plans:
            if frozen:
                value = await self.preparation.prepare_group(
                    frozen,
                    group,
                    call.settings,
                    question=request.question,
                    instructions=request.instructions,
                    schema=output_schema,
                    preview=True,
                )
                previews.append(value.to_dict())
            else:
                batch = await materialize(
                    self.preparer, group, call.settings.values.media.max_inline_bytes
                )
                previews.append(
                    self.executor.compiler.compile(
                        resource,
                        ReadingInput(request.question, request.instructions, batch.sources),
                        schema=output_schema,
                        language=call.settings.values.localization.language,
                    ).to_dict()
                )
        if len(previews) == 1 and isinstance(previews[0], dict):
            return previews[0]
        return {"batches": previews}

    async def probe(self, request: ProbeModel, call: CallContext) -> ModelOutcome:
        model = call.settings.values.models.get(request.model)
        if model is None:
            raise NousError(
                "model.unknown", "The model is not configured", details={"model": request.model}
            )
        frozen = FrozenTask(
            "model.probe",
            TaskAssignment(model=request.model, prompt="describe-media"),
            (ModelRoute(request.model, model, PresetConfig()),),
            self.executor.compiler.catalog.get("describe-media"),
        )
        selected = await self.preparation.accept_targets(
            (request.source or InlineText(text="Describe the supplied content briefly."),), call
        )
        plan = await self.preparation.prepare_input(frozen, selected, call.settings)
        generated = await self._compute_group(
            GenerationRequest(task="model.probe", inputs=selected),
            frozen,
            plan.input_plan.route_plans[0],
            call,
            list(plan.preparations),
        )
        result = await self.results.adopt(facts=generated.facts, outputs=generated.outputs)
        await call.saved(result.ref)
        return ModelOutcome(results=(result.ref,), coverage=result.coverage)

    async def discover(self, request: DiscoverModels, call: CallContext) -> dict[str, JsonValue]:
        return TypeAdapter(dict[str, JsonValue]).validate_python(
            {
                "provider": request.provider,
                "models": await self.executor.discover(request.provider, call.settings),
            }
        )
