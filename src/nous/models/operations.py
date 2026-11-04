"""Complete generation, template preview and model probes over shared content plans."""

from __future__ import annotations

import asyncio
import json
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, replace
from pathlib import Path
from typing import TYPE_CHECKING, cast

from pydantic import JsonValue, TypeAdapter

from nous.common.errors import NousError
from nous.common.operations import exception_context
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
    SourceSelection,
)
from nous.content.results import ResultStore
from nous.media.contracts import PreparedBatch, PreparedSource
from nous.models.configuration import PresetConfig, TaskAssignment
from nous.runtime.execution import ExecutionManager, OperationContext

from .contracts import GenerationResult, ReadingInput
from .executor import FrozenTask, ModelRoute, PreparedGeneration
from .input import InputPlan, RequestGroup, materialize, plan_inputs
from .tasks import TASKS

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
    transcription: FrozenTask | None = None
    input_plan: InputPlan | None = None


@dataclass(frozen=True)
class GenerationOutcome:
    artifacts: tuple[Artifact, ...]
    results: tuple[ProcessingResult, ...]
    coverage: dict[str, JsonValue]
    preparations: tuple[str, ...] = ()


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
    ) -> list[tuple[Artifact, SourceSelection]]:
        values = (
            [inputs]
            if isinstance(inputs, (InputSpec, Path, Artifact, ArtifactRef, str))
            else inputs
        )
        originals: list[tuple[Artifact, SourceSelection]] = []
        for value in values:
            if isinstance(value, Artifact):
                originals.append((value, SourceSelection()))
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
                originals.append((await self.results.artifacts.accept(spec), spec.selection))
        for ordinal, text in enumerate(texts, 1):
            originals.append(
                (
                    await self.results.artifacts.accept(
                        InputSpec(kind="text", text=text, display_name=f"text-{ordinal}.txt")
                    ),
                    SourceSelection(),
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
            transcription = self.freeze_transcription(snapshot)
            originals = await self._accept(input, texts)
            unavailable: dict[str, JsonValue] = {}
            for route_index, route in enumerate(frozen.routes):
                try:
                    plan = await plan_inputs(
                        self.preparer,
                        originals,
                        frozen,
                        snapshot.typed_values().media,
                        requirements=route.model.input_formats,
                        selection=selection,
                        document_mode=document_mode,
                        without_audio=without_audio,
                        per_batch=per_batch,
                    )
                    frozen = replace(
                        frozen, routes=frozen.routes[route_index:], ineligible_routes=unavailable
                    )
                    break
                except NousError as error:
                    if error.code not in {
                        "media.format_unavailable",
                        "media.vision_required",
                        "media.audio_capability_missing",
                        "media.jpeg_background_required",
                    }:
                        raise
                    unavailable[route.model_id] = error.to_dict()
            else:
                raise NousError(
                    "model.no_eligible_route",
                    "No configured route accepts the requested representations",
                    details={"routes": unavailable},
                )
            return PreparedRun(
                tuple(artifact for artifact, _ in originals),
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
                transcription,
                plan,
            )

    async def _route_readings(
        self,
        group: RequestGroup,
        primary: ReadingInput,
        frozen: FrozenTask,
        snapshot: ConfigSnapshot,
        context: OperationContext,
        completed: list[str],
    ) -> dict[str, ReadingInput]:
        readings = {frozen.routes[0].model_id: primary}
        for route in frozen.routes[1:]:
            alternatives: list[PreparedBatch] = []
            try:
                for batch in group.batches:
                    if batch.conversion is None:
                        if not any(source.media for source in batch.sources):
                            alternatives.append(batch)
                            continue
                        for source in batch.sources:
                            if not source.media or source.group is None:
                                alternatives.append(
                                    replace(
                                        batch,
                                        sources=(source,),
                                        inputs=(source.group,) if source.group else (),
                                    )
                                )
                                continue
                            origins = tuple(
                                dict.fromkeys(
                                    origin.origin
                                    for part in source.group.parts
                                    for origin in part.origins
                                )
                            )
                            selected = origins or tuple(part.content for part in source.group.parts)
                            for content in selected:
                                plan = await self.preparer.prepare(
                                    content.artifact,
                                    content.selection,
                                    requirements=route.model.input_formats,
                                    settings=snapshot.typed_values().media,
                                )
                                prefix = source.key.split(":", 1)[0]
                                alternatives.extend(
                                    replace(item, source_prefix=prefix) for item in plan.batches
                                )
                        continue
                    recipe = batch.conversion
                    plan = await self.preparer.prepare(
                        recipe.artifact,
                        recipe.selection,
                        requirements=route.model.input_formats,
                        settings=recipe.settings,
                        document_mode=recipe.document_mode,
                        without_audio=recipe.without_audio,
                    )
                    alternatives.extend(
                        replace(item, source_prefix=batch.source_prefix) for item in plan.batches
                    )
                actual = await materialize(
                    self.preparer,
                    RequestGroup(tuple(alternatives), group.coverage),
                    snapshot.typed_values().media.max_inline_bytes,
                )
                completed.extend(actual.completed_result_refs)
                readings[route.model_id] = await self.language_sources(
                    ReadingInput(primary.question, primary.instructions, actual.sources),
                    replace(frozen, routes=(route,)),
                    snapshot,
                    context,
                    completed,
                )
            except NousError as error:
                if error.code not in {
                    "media.format_unavailable",
                    "media.vision_required",
                    "media.jpeg_background_required",
                    "model.joint_input_limit",
                    "media.audio_capability_missing",
                }:
                    raise
                # Executor rejects this route against the available primary representation.
        return readings

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
            inputs=(InputGroup(key=source.key, name=source.name, parts=(part,)),),
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
        transcription = transcription or self.freeze_transcription(snapshot)
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
        async with self.execution.operation("model.run", snapshot) as context:
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
                    batch = await materialize(
                        self.preparer, plan, snapshot.typed_values().media.max_inline_bytes
                    )
                    completed.extend(batch.completed_result_refs)
                    reading = await self.language_sources(
                        ReadingInput(question, instructions, batch.sources),
                        prepared.frozen,
                        snapshot,
                        context,
                        completed,
                        prepared.transcription,
                    )
                    request = replace(
                        self.executor.prepare(
                            prepared.frozen,
                            reading,
                            snapshot,
                            schema=schema,
                            route_readings=await self._route_readings(
                                plan, reading, prepared.frozen, snapshot, context, completed
                            ),
                        ),
                        stage_refs=tuple(completed),
                    )
                    if on_prepared:
                        on_prepared(request)
                    result = await self.run_prepared(
                        request, coverage=batch.coverage, context=context, on_delta=on_delta
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
                if isinstance(exc, NousError):
                    exc.details.update(details)
                else:
                    exc.__dict__["nous_context"] = {**exception_context(exc), **details}
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
        context: OperationContext | None = None,
        schema: dict[str, JsonValue] | None = None,
        transcription: FrozenTask | None = None,
    ) -> ProcessingResult:
        async with self.execution.operation("model.generate", snapshot) as active:
            frozen = self.executor.freeze(task, snapshot) if isinstance(task, str) else task
            completed: list[str] = []
            try:
                reading = await self.language_sources(
                    reading, frozen, snapshot, context or active, completed, transcription
                )
                # Actual language representations determine adoption, including newly created transcripts.
                return await self.run_prepared(
                    replace(
                        self.executor.prepare(
                            frozen,
                            reading,
                            snapshot,
                            schema=schema,
                            route_readings=await self._route_readings(
                                RequestGroup(
                                    (
                                        PreparedBatch(
                                            reading.sources,
                                            tuple(
                                                source.group
                                                for source in reading.sources
                                                if source.group
                                            ),
                                            SourceSelection(),
                                            TypeAdapter(dict[str, JsonValue]).validate_python(
                                                coverage or {}
                                            ),
                                        ),
                                    ),
                                    TypeAdapter(dict[str, JsonValue]).validate_python(
                                        coverage or {}
                                    ),
                                ),
                                reading,
                                frozen,
                                snapshot,
                                context or active,
                                completed,
                            ),
                        ),
                        stage_refs=tuple(completed),
                    ),
                    inputs=tuple(source.group for source in reading.sources if source.group)
                    if completed
                    else inputs,
                    coverage=coverage,
                    context=context or active,
                )
            except NousError as exc:
                exc.details["completed_result_refs"] = [str(ref) for ref in completed]
                raise

    async def run_prepared(
        self,
        prepared: PreparedGeneration,
        *,
        inputs: Sequence[InputGroup] | None = None,
        coverage: Mapping[str, object] | None = None,
        context: OperationContext | None = None,
        on_delta: Callable[[str], None] | None = None,
    ) -> ProcessingResult:
        async with self.execution.operation("model.generate", prepared.snapshot) as active:
            if on_delta is None:
                generated = await self.executor.execute(prepared, context=context or active)
            else:
                async with self.executor.stream(prepared, context=context or active) as stream:
                    async for delta in stream:
                        on_delta(delta)
                    if stream.result is None:
                        raise NousError(
                            "model.incomplete_stream", "The stream returned no completed result"
                        )
                    generated = stream.result
            return await self._adopt(prepared, generated, inputs, coverage)

    async def _adopt(
        self,
        prepared: PreparedGeneration,
        generated: GenerationResult,
        inputs: Sequence[InputGroup] | None,
        coverage: Mapping[str, object] | None,
    ) -> ProcessingResult:
        reading = prepared.route_compiled.get(
            str(generated.producer.get("model_id")), prepared.compiled
        ).reading
        definition = TASKS[prepared.compiled.resource.manifest.contract_id]
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
        return await self.results.adopt(
            operation=prepared.task,
            producer=generated.producer,
            inputs=tuple(inputs)
            if inputs is not None and reading is prepared.compiled.reading
            else tuple(source.group for source in reading.sources if source.group),
            outputs=outputs,
            request=generated.request,
            parameters={
                "attempts": generated.attempts,
                "usage": generated.usage,
                "finish_reason": generated.finish_reason,
                "stage_result_refs": list(prepared.stage_refs),
            },
            coverage=coverage,
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
                        InputSpec(kind="artifact", artifact=artifact.ref, selection=selected)
                        for artifact, selected in originals
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
                    frozen,
                    snapshot.typed_values().media,
                    requirements=frozen.routes[0].model.input_formats if frozen else (),
                    per_batch=per_batch,
                )
            previews: list[JsonValue] = []
            for group in plan.request_groups:
                batch = await materialize(
                    self.preparer, group, snapshot.typed_values().media.max_inline_bytes
                )
                reading = ReadingInput(question, instructions, batch.sources)
                accepted = frozen.routes[0].model.input_formats if frozen else ()
                missing = [
                    medium.media_type
                    for source in reading.sources
                    for medium in source.media
                    if medium.media_type.startswith("audio/") and medium.media_type not in accepted
                ]
                if missing:
                    preview_missing: dict[str, JsonValue] = {
                        "status": "requires_transcription",
                        "formats": list[JsonValue](missing),
                        "source_groups": list[JsonValue](
                            [item.model_dump(mode="json") for item in batch.inputs]
                        ),
                        "transcription_task": snapshot.typed_values().media.audio.transcription_task,
                    }
                    previews.append(preview_missing)
                    continue
                compiled = (
                    self.executor.prepare(frozen, reading, snapshot, schema=schema).compiled
                    if frozen
                    else self.executor.compiler.compile(resource, reading, schema=schema)
                )
                previews.append(compiled.to_dict())
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
