"""Complete generation, template preview and model probes over shared content plans."""

from __future__ import annotations

import asyncio
import json
from collections.abc import Awaitable, Callable, Mapping, Sequence
from dataclasses import dataclass, replace
from pathlib import Path
from typing import TYPE_CHECKING, cast

from pydantic import JsonValue, TypeAdapter

from nous.common.errors import NousError
from nous.common.operations import add_error_context, record_completed
from nous.content.contracts import (
    Artifact,
    ArtifactRef,
    ContentSelection,
    InputGroup,
    InputPart,
    InputSpec,
    OriginSlice,
    OutputContent,
    ProcessingResult,
    SourceRole,
    SourceSelection,
)
from nous.content.results import ResultStore
from nous.media.contracts import PreparedBatch, PreparedSource
from nous.models.configuration import PresetConfig, TaskAssignment
from nous.runtime.execution import ExecutionManager, OperationContext

from .contracts import GenerationResult, ReadingInput
from .executor import FrozenTask, ModelRoute, PreparedGeneration
from .input import InputPlan, RequestGroup, for_route, materialize, plan_inputs
from .policy import route_unavailable

if TYPE_CHECKING:
    from nous.configuration import ConfigSnapshot
    from nous.media.preparation import MediaPreparer

    from .executor import ModelExecutor

InputValue = InputSpec | Path | Artifact | ArtifactRef | str


@dataclass(frozen=True)
class PreparedRun:
    artifacts: tuple[Artifact, ...]
    batches: tuple[RequestGroup, ...]
    coverage: dict[str, JsonValue]
    frozen: FrozenTask
    question: str = ""
    instructions: str = ""
    schema: dict[str, JsonValue] | None = None
    preparations: tuple[str, ...] = ()
    input_plan: InputPlan | None = None


@dataclass(frozen=True)
class GenerationOutcome:
    artifacts: tuple[Artifact, ...]
    results: tuple[ProcessingResult, ...]
    coverage: dict[str, JsonValue]
    preparations: tuple[str, ...] = ()


@dataclass(frozen=True)
class ProducedTask:
    """A successful language computation whose named outputs can be adopted without another call."""

    operation: str
    producer: dict[str, JsonValue]
    actual_inputs: tuple[InputGroup, ...]
    outputs: dict[str, OutputContent]
    request: dict[str, JsonValue]
    parameters: dict[str, JsonValue]
    coverage: dict[str, JsonValue]
    completed_stage_refs: tuple[str, ...]


@dataclass(frozen=True)
class PendingGeneration:
    frozen: FrozenTask
    reading: ReadingInput
    coverage: dict[str, JsonValue]
    formats: tuple[str, ...]
    transcription_task: str | None

    def to_dict(self) -> dict[str, JsonValue]:
        return {
            "status": "requires_transcription",
            "task": self.frozen.task,
            "model_id": self.frozen.routes[0].model_id,
            "formats": list(self.formats),
            "transcription_task": self.transcription_task,
            "source_groups": [
                source.group.model_dump(mode="json")
                for source in self.reading.sources
                if source.group
            ],
            "coverage": self.coverage,
            "deferred_routes": [route.model_id for route in self.frozen.routes[1:]],
        }


class GenerationOperations:
    """Own admission, language stages and adoption; prepare only the active bounded request."""

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

    def capabilities(self, snapshot: ConfigSnapshot) -> dict[str, JsonValue]:
        tasks: dict[str, JsonValue] = {}
        for name in snapshot.typed_values().tasks:
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

    def freeze_transcription(self, snapshot: ConfigSnapshot) -> FrozenTask | None:
        task = snapshot.typed_values().media.audio.transcription_task
        return self.executor.freeze(task, snapshot) if task else None

    async def _accept(
        self, inputs: InputValue | Sequence[InputValue], texts: Sequence[str]
    ) -> list[tuple[Artifact, SourceSelection, SourceRole]]:
        values = (
            [inputs]
            if isinstance(inputs, (InputSpec, Path, Artifact, ArtifactRef, str))
            else inputs
        )
        originals: list[tuple[Artifact, SourceSelection, SourceRole]] = []
        for value in values:
            if isinstance(value, Artifact):
                originals.append((value, SourceSelection(), "source"))
            else:
                spec = (
                    value
                    if isinstance(value, InputSpec)
                    else (
                        InputSpec(kind="file", path=str(value))
                        if isinstance(value, Path)
                        else InputSpec(kind="artifact", artifact=value)
                    )
                )
                originals.append(
                    (await self.results.artifacts.accept(spec), spec.selection, spec.role)
                )
        for ordinal, text in enumerate(texts, 1):
            originals.append(
                (
                    await self.results.artifacts.accept(
                        InputSpec(kind="text", text=text, display_name=f"text-{ordinal}.txt")
                    ),
                    SourceSelection(),
                    "source",
                )
            )
        if not originals:
            raise NousError("model.input_required", "Provide an input artifact, file or text")
        return originals

    async def prepare_input(
        self,
        task: str | FrozenTask,
        input: InputValue | Sequence[InputValue],
        snapshot: ConfigSnapshot,
        *,
        selection: SourceSelection | None = None,
        question: str = "",
        instructions: str = "",
        schema: dict[str, JsonValue] | None = None,
        document_mode: str = "auto",
        without_audio: bool = False,
        texts: Sequence[str] = (),
        per_batch: bool = False,
    ) -> PreparedRun:
        async with self.execution.operation("model.prepare_input", snapshot):
            frozen = self.executor.freeze(task, snapshot) if isinstance(task, str) else task
            originals = await self._accept(input, texts)
            unavailable: dict[str, JsonValue] = {}
            for route_index, route in enumerate(frozen.routes):
                try:
                    self.executor.validate_route(frozen, route, snapshot)
                    plan = await plan_inputs(
                        self.preparer,
                        originals,
                        snapshot.typed_values().media,
                        requirements=route.model.input_formats,
                        selection=selection,
                        document_mode=document_mode,
                        without_audio=without_audio,
                        per_batch=per_batch,
                        contract=self.executor.compiler.catalog.tasks[
                            frozen.resource.manifest.contract_id
                        ]
                        if frozen.resource
                        else None,
                    )
                    frozen = replace(
                        frozen, routes=frozen.routes[route_index:], ineligible_routes=unavailable
                    )
                    break
                except NousError as error:
                    if not route_unavailable(error):
                        raise
                    unavailable[route.model_id] = error.to_dict()
            else:
                raise NousError(
                    "model.no_eligible_route",
                    "No configured route accepts the requested representations",
                    details={"routes": unavailable},
                )
            return PreparedRun(
                tuple(artifact for artifact, _, _ in originals),
                plan.request_groups,
                plan.requested_coverage,
                frozen,
                question,
                instructions,
                schema,
                tuple(
                    dict.fromkeys(
                        ref
                        for item in plan.representation_plans
                        for ref in item.completed_result_refs
                    )
                ),
                plan,
            )

    async def prepare_group(
        self,
        frozen: FrozenTask,
        group: RequestGroup,
        snapshot: ConfigSnapshot,
        *,
        question: str = "",
        instructions: str = "",
        schema: dict[str, JsonValue] | None = None,
        extra_sources: Sequence[PreparedSource] = (),
        preview: bool = False,
        streaming: bool = False,
        route: ModelRoute | None = None,
        completed: list[str] | None = None,
        on_prepared: Callable[[PreparedGeneration], None] | None = None,
    ) -> PreparedGeneration | PendingGeneration:
        """Select and prepare one route; unused alternatives perform no conversion or language work."""
        completed = completed if completed is not None else []
        unavailable = dict(frozen.ineligible_routes)
        candidates = (route,) if route is not None else frozen.routes
        async with self.execution.operation("model.prepare_route", snapshot) as context:
            for candidate in candidates:
                try:
                    self.executor.validate_route(frozen, candidate, snapshot, streaming=streaming)
                    selected = replace(
                        frozen,
                        routes=frozen.routes[frozen.routes.index(candidate) :],
                        ineligible_routes=unavailable,
                    )
                    batch = await materialize(
                        self.preparer,
                        await for_route(
                            self.preparer,
                            group,
                            candidate.model.input_formats,
                            snapshot.typed_values().media,
                        ),
                        snapshot.typed_values().media.max_inline_bytes,
                    )
                    completed.extend(batch.completed_result_refs)
                    reading = ReadingInput(question, instructions, (*batch.sources, *extra_sources))
                    missing = tuple(
                        medium.media_type
                        for source in reading.sources
                        for medium in source.media
                        if medium.media_type.startswith("audio/")
                        and medium.media_type not in candidate.model.input_formats
                    )
                    if preview and missing:
                        return PendingGeneration(
                            selected,
                            reading,
                            batch.coverage,
                            missing,
                            snapshot.typed_values().media.audio.transcription_task,
                        )
                    reading = await self.language_sources(
                        reading, selected, snapshot, context, completed
                    )
                    prepared = replace(
                        self.executor.prepare(selected, reading, snapshot, schema=schema),
                        stage_refs=tuple(dict.fromkeys(completed)),
                        coverage=batch.coverage,
                    )
                    if on_prepared:
                        on_prepared(prepared)
                    return prepared
                except NousError as error:
                    if not route_unavailable(error):
                        raise
                    error.details["preparation_route"] = candidate.model_id
                    unavailable[candidate.model_id] = error.to_dict()
                    if route is not None:
                        raise
            raise NousError(
                "model.no_eligible_route",
                "No configured route can prepare the requested inputs",
                details={"routes": unavailable},
            )

    async def run_group(
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
    ) -> ProcessingResult:
        async with self.execution.operation("model.generate", snapshot):
            produced = await self.produce(
                frozen,
                group,
                snapshot,
                question=question,
                instructions=instructions,
                schema=schema,
                extra_sources=extra_sources,
                completed=completed,
                on_delta=on_delta,
                on_prepared=on_prepared,
            )
            return await self.adopt_produced(produced)

    async def produce(
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
    ) -> ProducedTask:
        """Run one semantic group; the executor requests another representation only when switching."""
        completed = completed if completed is not None else []
        async with self.execution.operation("model.generate", snapshot):

            async def resolve(route: ModelRoute | None = None) -> PreparedGeneration:
                request = await self.prepare_group(
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

            return await self.produce_prepared(
                await resolve(), on_delta=on_delta, prepare_route=resolve
            )

    async def transcribe(
        self,
        source: PreparedSource,
        part: InputPart,
        snapshot: ConfigSnapshot,
        context: OperationContext,
        frozen: FrozenTask | None = None,
    ) -> ProcessingResult:
        """Adopt exactly one successful endpoint response, with segment-local body coordinates."""
        task = snapshot.typed_values().media.audio.transcription_task
        if task is None:
            raise NousError(
                "media.audio_capability_missing",
                "Audio requires native input or a configured transcription task",
            )
        artifact = await self.results.artifacts.get(part.content.artifact)
        generated = await self.executor.transcribe(
            frozen or task,
            await self.results.artifacts.read_bytes(artifact.ref),
            artifact.display_name,
            snapshot,
            context=context,
        )
        if not isinstance(generated.output, str):
            raise NousError("media.invalid_transcript", "Transcription returned a non-text result")
        return await self.results.adopt(
            operation=task,
            producer=generated.producer,
            inputs=(
                InputGroup(
                    key=source.key,
                    name=source.name,
                    role=source.group.role if source.group else "source",
                    parts=(part,),
                ),
            ),
            outputs={
                "transcript": OutputContent(
                    data=generated.output.encode(),
                    media_type="text/plain",
                    display_name="transcript.txt",
                )
            },
            request=generated.request,
            parameters={"usage": generated.usage, "attempts": generated.attempts},
            coverage={
                "mode": "transcript",
                "origins": [origin.model_dump(mode="json") for origin in part.origins],
            },
        )

    async def language_sources(
        self,
        reading: ReadingInput,
        frozen: FrozenTask,
        snapshot: ConfigSnapshot,
        context: OperationContext,
        completed: list[str],
        transcription: FrozenTask | None = None,
    ) -> ReadingInput:
        """Resolve language-dependent representations after deterministic preparation has finished."""
        sources: list[PreparedSource] = []
        accepted = frozen.routes[0].model.input_formats
        for source in reading.sources:
            if source.group is None or not any(
                media.media_type.startswith("audio/") and media.media_type not in accepted
                for media in source.media
            ):
                sources.append(source)
                continue
            parts: list[InputPart] = []
            texts = [source.text] if source.text else []
            media = [
                medium for medium in source.media if not medium.media_type.startswith("audio/")
            ]
            for part in source.group.parts:
                artifact = await self.results.artifacts.get(part.content.artifact)
                if not artifact.media_type.startswith("audio/") or artifact.media_type in accepted:
                    parts.append(part)
                    continue
                transcription = transcription or self.freeze_transcription(snapshot)
                result = await self.transcribe(source, part, snapshot, context, transcription)
                completed.append(result.ref.address)
                output = next(output for output in result.outputs if output.name == "transcript")
                texts.append(await self.results.artifacts.read_text(output.artifact))
                parts.append(
                    InputPart(
                        content=ContentSelection(artifact=output.artifact),
                        representation="transcript",
                        media_type="text/plain",
                        source_media_type=part.source_media_type or part.media_type,
                        origins=(OriginSlice(origin=part.content, transform_ref=result.ref),),
                    )
                )
            group = source.group.model_copy(update={"parts": tuple(parts)})
            sources.append(replace(source, text="\n".join(texts), media=tuple(media), group=group))
        return replace(reading, sources=tuple(sources))

    async def run_input(
        self,
        task: str | FrozenTask,
        input: InputValue | Sequence[InputValue],
        snapshot: ConfigSnapshot,
        *,
        selection: SourceSelection | None = None,
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
                selection=selection,
                question=question,
                instructions=instructions,
                schema=schema,
                document_mode=document_mode,
                without_audio=without_audio,
                texts=texts,
                per_batch=per_batch,
            )
            completed.extend(prepared.preparations)
            try:
                for plan in prepared.batches:
                    result = await self.run_group(
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
                    results.append(result)
                    completed.append(result.ref.address)
                    if on_completed:
                        on_completed(result)
            except (NousError, asyncio.CancelledError, TimeoutError) as exc:
                details: dict[str, JsonValue] = {
                    "artifacts": [artifact.ref.address for artifact in prepared.artifacts],
                    "completed_result_refs": list[JsonValue](completed),
                    "unfinished_coverage": [
                        batch.coverage for batch in prepared.batches[len(results) :]
                    ],
                }
                record_completed(completed)
                add_error_context(exc, details)
                raise
            return GenerationOutcome(
                prepared.artifacts,
                tuple(results),
                {**prepared.coverage, "actual": [result.coverage for result in results]},
                tuple(completed),
            )

    async def run(
        self,
        task: str | FrozenTask,
        reading: ReadingInput,
        snapshot: ConfigSnapshot,
        *,
        inputs: Sequence[InputGroup] | None = None,
        coverage: Mapping[str, object] | None = None,
        schema: dict[str, JsonValue] | None = None,
    ) -> ProcessingResult:
        frozen = self.executor.freeze(task, snapshot) if isinstance(task, str) else task
        actual_coverage = TypeAdapter(dict[str, JsonValue]).validate_python(coverage or {})
        batch = PreparedBatch(
            reading.sources,
            tuple(inputs)
            if inputs is not None
            else tuple(source.group for source in reading.sources if source.group),
            SourceSelection(),
            actual_coverage,
        )
        return await self.run_group(
            frozen,
            RequestGroup((batch,), actual_coverage),
            snapshot,
            question=reading.question,
            instructions=reading.instructions,
            schema=schema,
        )

    async def run_prepared(
        self,
        prepared: PreparedGeneration,
        *,
        inputs: Sequence[InputGroup] | None = None,
        coverage: Mapping[str, object] | None = None,
        context: OperationContext | None = None,
        on_delta: Callable[[str], None] | None = None,
        prepare_route: Callable[[ModelRoute], Awaitable[PreparedGeneration]] | None = None,
    ) -> ProcessingResult:
        async with self.execution.operation("model.generate", prepared.snapshot):
            return await self.adopt_produced(
                await self.produce_prepared(
                    prepared,
                    inputs=inputs,
                    coverage=coverage,
                    context=context,
                    on_delta=on_delta,
                    prepare_route=prepare_route,
                )
            )

    async def adopt_produced(self, produced: ProducedTask) -> ProcessingResult:
        """Save a successful computation; SQL failures never re-enter model execution."""
        published = None
        try:
            published = await self.results.prepare_outputs(produced.outputs)
            async with self.results.database.write() as tx:
                result = await self.results.adopt_in(
                    tx,
                    operation=produced.operation,
                    producer=produced.producer,
                    inputs=produced.actual_inputs,
                    prepared_outputs=published,
                    request=produced.request,
                    parameters=produced.parameters,
                    coverage=produced.coverage,
                )
            record_completed(result.ref.address)
            return result
        except BaseException as error:
            add_error_context(
                error,
                {
                    "stage": "result.adopt",
                    "model_succeeded": True,
                    "completed_result_refs": list(produced.completed_stage_refs),
                    "published_outputs": [
                        {"name": output.name, "blob": output.content.blob.model_dump(mode="json")}
                        for output in published.outputs
                    ]
                    if published is not None
                    else [],
                },
                recovery=produced,
            )
            raise

    async def produce_prepared(
        self,
        prepared: PreparedGeneration,
        *,
        inputs: Sequence[InputGroup] | None = None,
        coverage: Mapping[str, object] | None = None,
        context: OperationContext | None = None,
        on_delta: Callable[[str], None] | None = None,
        prepare_route: Callable[[ModelRoute], Awaitable[PreparedGeneration]] | None = None,
    ) -> ProducedTask:
        async with self.execution.operation("model.generate", prepared.snapshot) as active:
            if on_delta is None:
                generated = await self.executor.execute(
                    prepared, context=context or active, prepare_route=prepare_route
                )
            else:
                async with self.executor.stream(
                    prepared, context=context or active, prepare_route=prepare_route
                ) as stream:
                    async for delta in stream:
                        on_delta(delta)
                    if stream.result is None:
                        raise NousError(
                            "model.incomplete_stream", "The stream returned no completed result"
                        )
                    generated = stream.result
            return await self._interpret(prepared, generated, inputs, coverage)

    async def _interpret(
        self,
        prepared: PreparedGeneration,
        generated: GenerationResult,
        inputs: Sequence[InputGroup] | None,
        coverage: Mapping[str, object] | None,
    ) -> ProducedTask:
        reading = generated.reading or prepared.compiled.reading
        definition = self.executor.compiler.catalog.tasks[
            prepared.compiled.resource.manifest.contract_id
        ]
        outputs = await definition.outputs(
            generated.output,
            reading,
            self.results.artifacts,
            structured=prepared.compiled.schema is not None,
            producer=generated.producer,
        )
        if generated.response:
            outputs["response"] = OutputContent(
                data=json.dumps(generated.response, ensure_ascii=False, indent=2).encode(),
                media_type="application/json",
                display_name="response.json",
            )
        stages = tuple(
            cast(list[str], generated.request.get("stage_result_refs", list(prepared.stage_refs)))
        )
        return ProducedTask(
            operation=prepared.task,
            producer=generated.producer,
            actual_inputs=tuple(inputs)
            if inputs is not None and reading is prepared.compiled.reading
            else tuple(source.group for source in reading.sources if source.group),
            outputs=outputs,
            request=generated.request,
            parameters={
                "attempts": generated.attempts,
                "usage": generated.usage,
                "finish_reason": generated.finish_reason,
                "stage_result_refs": generated.request.get(
                    "stage_result_refs", list(prepared.stage_refs)
                ),
            },
            coverage=generated.coverage
            or TypeAdapter(dict[str, JsonValue]).validate_python(coverage or {}),
            completed_stage_refs=stages,
        )

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
        async with self.execution.operation("prompt.preview", snapshot):
            resource = self.executor.compiler.catalog.get(prompt)
            frozen = self.executor.freeze(task, snapshot) if task else None
            if frozen is not None and (
                frozen.resource is None or frozen.resource.id != resource.id
            ):
                raise NousError(
                    "model.prompt_conflict",
                    "Prompt preview must use the selected task's prompt resource",
                )
            originals = await self._accept(input, texts) if input or texts else []
            if frozen and originals:
                prepared = await self.prepare_input(
                    frozen,
                    [
                        InputSpec(
                            kind="artifact", artifact=artifact.ref, selection=selected, role=role
                        )
                        for artifact, selected, role in originals
                    ],
                    snapshot,
                    per_batch=per_batch,
                )
                assert prepared.input_plan is not None
                plan, frozen = prepared.input_plan, prepared.frozen
            else:
                plan = await plan_inputs(
                    self.preparer,
                    originals,
                    snapshot.typed_values().media,
                    requirements=frozen.routes[0].model.input_formats if frozen else (),
                    per_batch=per_batch,
                    contract=self.executor.compiler.catalog.tasks[resource.manifest.contract_id],
                )
            previews: list[JsonValue] = []
            for group in plan.request_groups:
                if frozen:
                    request = await self.prepare_group(
                        frozen,
                        group,
                        snapshot,
                        question=question,
                        instructions=instructions,
                        schema=schema,
                        preview=True,
                    )
                    previews.append(request.to_dict())
                else:
                    batch = await materialize(
                        self.preparer, group, snapshot.typed_values().media.max_inline_bytes
                    )
                    previews.append(
                        self.executor.compiler.compile(
                            resource,
                            ReadingInput(question, instructions, batch.sources),
                            schema=schema,
                        ).to_dict()
                    )
            if len(previews) == 1:
                return dict(cast(dict[str, JsonValue], previews[0]))
            return {"batches": previews, "representation": "task" if task else "template"}

    async def probe(
        self, model: str, input: Path | None, snapshot: ConfigSnapshot
    ) -> GenerationOutcome:
        settings = snapshot.typed_values()
        selected = settings.models.get(model)
        if selected is None:
            raise NousError(
                "model.unknown", "The model is not configured", details={"model": model}
            )
        assignment = TaskAssignment(model=model, prompt="describe-media")
        frozen = FrozenTask(
            "model.probe",
            assignment,
            (ModelRoute(model, selected, PresetConfig()),),
            self.executor.compiler.catalog.get("describe-media"),
        )
        return await self.run_input(
            frozen,
            [input] if input else [],
            snapshot,
            texts=() if input else ("Describe the supplied content briefly.",),
        )
