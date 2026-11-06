"""Complete generation, template preview and model probes over shared content plans."""

from __future__ import annotations

import asyncio
import hashlib
import json
from collections.abc import Awaitable, Callable, Mapping, Sequence
from dataclasses import dataclass, replace
from pathlib import Path
from typing import TYPE_CHECKING, cast
from uuid import uuid4

from pydantic import JsonValue, TypeAdapter

from nous.common.errors import NousError
from nous.common.operations import add_error_context, record_completed
from nous.content.contracts import (
    Artifact,
    ArtifactRef,
    BlobRef,
    ContentSelection,
    InputGroup,
    InputPart,
    InputSpec,
    OriginSlice,
    OutputContent,
    ProcessingResult,
    ResultRef,
    SourceRole,
    SourceSelection,
)
from nous.content.results import ResultStore
from nous.media.contracts import PreparedBatch, PreparedSource
from nous.models.configuration import PresetConfig, TaskAssignment
from nous.runtime.execution import ExecutionManager, OperationContext

from .computation import (
    FrozenLanguageTask,
    PublishedComputation,
    descriptor_text,
    freeze_task,
    generated_value,
    load_task,
    read_generated,
    reading_value,
)
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
    """Coordinate language preparation, execution and result adoption.

    Only the active bounded request is materialized. Use ``produce`` when a
    caller must combine a completed model output with its own adoption; use
    ``run_group`` to execute and adopt a group directly. Dependencies are
    borrowed from the runtime.
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

    async def freeze_language_task(self, task: str, snapshot: ConfigSnapshot) -> FrozenLanguageTask:
        frozen = self.executor.freeze(task, snapshot)
        contract = (
            self.executor.compiler.catalog.tasks[
                frozen.resource.manifest.contract_id
            ].describe_contract()
            if frozen.resource
            else {}
        )
        transcription_ref = None
        transcription_task = snapshot.values.media.audio.transcription_task
        if (
            transcription_task
            and transcription_task != task
            and frozen.assignment.operation != "transcription"
        ):
            transcription = self.executor.freeze(transcription_task, snapshot)
            transcription_contract = (
                self.executor.compiler.catalog.tasks[
                    transcription.resource.manifest.contract_id
                ].describe_contract()
                if transcription.resource
                else {}
            )
            transcription_ref = (
                await freeze_task(self.results, transcription, snapshot, transcription_contract)
            ).descriptor
        return await freeze_task(self.results, frozen, snapshot, contract, transcription_ref)

    async def load_language_task(
        self, ref: ArtifactRef | FrozenLanguageTask, snapshot: ConfigSnapshot
    ) -> FrozenTask:
        return await load_task(
            self.results,
            ref.descriptor if isinstance(ref, FrozenLanguageTask) else ref,
            snapshot,
            self.executor.compiler.catalog.tasks,
        )

    async def publish_computation(
        self, produced: ProducedTask, producer_key: str, frozen_task_ref: ArtifactRef | None = None
    ) -> PublishedComputation:
        published = await self.results.prepare_outputs(produced.outputs)
        value = {
            "producer_key": producer_key,
            "operation": produced.operation,
            "producer": produced.producer,
            "actual_inputs": [group.model_dump(mode="json") for group in produced.actual_inputs],
            "outputs": {
                output.name: {
                    "blob": output.content.blob.model_dump(mode="json"),
                    "media_type": output.media_type,
                    "display_name": output.display_name,
                }
                for output in published.outputs
            },
            "request": produced.request,
            "parameters": produced.parameters,
            "coverage": produced.coverage,
            "completed_stage_refs": list(produced.completed_stage_refs),
            "frozen_task_ref": frozen_task_ref.model_dump(mode="json") if frozen_task_ref else None,
        }
        outputs = {
            **produced.outputs,
            "computation_descriptor": OutputContent(
                data=json.dumps(value, sort_keys=True, ensure_ascii=False).encode(),
                media_type="application/json",
                display_name="computation.json",
            ),
        }
        result = await self.results.adopt(
            operation="model.published_computation",
            producer=produced.producer,
            inputs=produced.actual_inputs,
            outputs=outputs,
            request=produced.request,
            parameters={"stage_result_refs": list(produced.completed_stage_refs)},
            coverage=produced.coverage,
            producer_key=producer_key,
        )
        return PublishedComputation(
            descriptor=next(
                output.artifact
                for output in result.outputs
                if output.name == "computation_descriptor"
            ),
            producer_key=producer_key,
        )

    async def load_computation(self, value: PublishedComputation | ArtifactRef) -> ProducedTask:
        descriptor = value.descriptor if isinstance(value, PublishedComputation) else value
        data = json.loads(await descriptor_text(self.results, descriptor))
        if (
            isinstance(value, PublishedComputation)
            and data.get("producer_key") != value.producer_key
        ):
            raise NousError(
                "content.producer_conflict",
                "Computation descriptor belongs to a different producer key",
            )
        outputs: dict[str, OutputContent] = {}
        for name, output in data["outputs"].items():
            blob = BlobRef.model_validate(output["blob"])
            outputs[name] = OutputContent(
                data=await self.results.artifacts.content.read_bytes(
                    blob, max_bytes=max(blob.byte_length, 1)
                ),
                media_type=output["media_type"],
                display_name=output["display_name"],
            )
        return ProducedTask(
            operation=data["operation"],
            producer=data["producer"],
            actual_inputs=tuple(
                InputGroup.model_validate(group) for group in data["actual_inputs"]
            ),
            outputs=outputs,
            request=data["request"],
            parameters=data["parameters"],
            coverage=data["coverage"],
            completed_stage_refs=tuple(data["completed_stage_refs"]),
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

    def freeze_transcription(self, snapshot: ConfigSnapshot) -> FrozenTask | None:
        task = snapshot.values.media.audio.transcription_task
        return self.executor.freeze(task, snapshot) if task else None

    async def _accept(
        self, inputs: InputValue | Sequence[InputValue], texts: Sequence[str]
    ) -> list[tuple[Artifact, SourceSelection, SourceRole, str | None]]:
        values = (
            [inputs]
            if isinstance(inputs, (InputSpec, Path, Artifact, ArtifactRef, str))
            else inputs
        )
        originals: list[tuple[Artifact, SourceSelection, SourceRole, str | None]] = []
        for value in values:
            if isinstance(value, Artifact):
                originals.append((value, SourceSelection(), "source", None))
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
                    (
                        await self.results.artifacts.accept(spec),
                        spec.selection,
                        spec.role,
                        spec.slot_key,
                    )
                )
        for ordinal, text in enumerate(texts, 1):
            originals.append(
                (
                    await self.results.artifacts.accept(
                        InputSpec(kind="text", text=text, display_name=f"text-{ordinal}.txt")
                    ),
                    SourceSelection(),
                    "source",
                    None,
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
                        (frozen.media_settings or snapshot.values.media),
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
                tuple(artifact for artifact, _, _, _ in originals),
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
                            (frozen.media_settings or snapshot.values.media),
                        ),
                        min(
                            (frozen.media_settings or snapshot.values.media).max_inline_bytes,
                            snapshot.values.media.max_inline_bytes,
                        ),
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
                            snapshot.values.media.audio.transcription_task,
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
        """Produce and adopt one semantic group using the supplied snapshot.

        Model computation and adoption are separate stages. When composing an
        atomic adoption with another owner, use ``produce`` and adopt its output
        within the caller's transaction instead.

        Returns:
            The committed processing result for the group.

        Raises:
            NousError: If preparation, model execution or result adoption fails.
                An adoption failure retains successful computation as recovery
                context on the error.
        """
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
        producer_key: str | None = None,
        frozen_task_ref: ArtifactRef | None = None,
    ) -> ProducedTask:
        """Compute one semantic group without adopting its named outputs.

        The executor requests another representation only when switching routes.
        The returned ``ProducedTask`` retains the successful output and its actual
        inputs; pass it to ``adopt_produced`` or a caller-owned adoption without
        making another model request. Completed preparation stages may already
        have been adopted before this method returns or fails.

        Returns:
            The successful output, actual inputs and completed preparation
            references, ready for adoption.

        Raises:
            NousError: If no eligible route can prepare or complete the request.
        """
        completed = completed if completed is not None else []
        async with self.execution.operation("model.generate", snapshot):
            identity = hashlib.sha256(
                json.dumps(
                    {
                        "task": frozen.task,
                        "prompt": frozen.resource.execution_signature if frozen.resource else None,
                        "question": question,
                        "instructions": instructions,
                        "schema": schema,
                        "inputs": [
                            [item.model_dump(mode="json") for item in batch.inputs]
                            for batch in group.batches
                        ],
                        "extra_sources": reading_value(ReadingInput(sources=tuple(extra_sources))),
                    },
                    sort_keys=True,
                    ensure_ascii=False,
                ).encode()
            ).hexdigest()
            if producer_key is not None:
                accepted = await self.results.find_producer(producer_key + ":response")
                if accepted is not None:
                    if accepted.parameters.get("request_identity") != identity:
                        raise NousError(
                            "content.producer_conflict",
                            "Saved response belongs to a different language request",
                            details={"producer_key": producer_key},
                        )
                    envelope = next(
                        output.artifact
                        for output in accepted.outputs
                        if output.name == "computation"
                    )
                    generated = read_generated(
                        json.loads(await descriptor_text(self.results, envelope))
                    )
                    assert generated.reading is not None
                    prepared = self.executor.prepare(
                        frozen, generated.reading, snapshot, schema=schema
                    )
                    return await self._interpret_response(
                        prepared,
                        generated,
                        accepted,
                        producer_key + ":response",
                        None,
                        generated.coverage,
                    )

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
                await resolve(),
                on_delta=on_delta,
                prepare_route=resolve,
                producer_key=producer_key,
                frozen_task_ref=frozen_task_ref,
                request_identity=identity,
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
        task = frozen.task if frozen is not None else snapshot.values.media.audio.transcription_task
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
                    slot_key=source.group.slot_key if source.group else source.key,
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
            parameters={
                "usage": generated.usage,
                "attempts": generated.attempts,
                "representations": {"transcript": {"kind": "transcript"}},
            },
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
                transcription = transcription or frozen.transcription
                if transcription is None:
                    if frozen.media_settings is not None:
                        raise NousError(
                            "media.audio_capability_missing",
                            "The accepted task has no configured transcription resource",
                        )
                    transcription = self.freeze_transcription(snapshot)
                result = await self.transcribe(source, part, snapshot, context, transcription)
                completed.append(result.ref.identifier)
                output = next(output for output in result.outputs if output.name == "transcript")
                texts.append(await self.results.artifacts.read_text(output.artifact))
                parts.append(
                    InputPart(
                        content=ContentSelection(artifact=output.artifact),
                        representation="transcript",
                        media_type="text/plain",
                        source_media_type=part.source_media_type or part.media_type,
                        origins=tuple(
                            OriginSlice(origin=origin, transform_ref=result.ref)
                            for origin in (
                                tuple(item.origin for item in part.origins) or (part.content,)
                            )
                        ),
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
                    completed.append(result.ref.identifier)
                    if on_completed:
                        on_completed(result)
            except (NousError, asyncio.CancelledError, TimeoutError) as exc:
                details: dict[str, JsonValue] = {
                    "artifacts": [artifact.ref.identifier for artifact in prepared.artifacts],
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

    async def adopt_produced(
        self, produced: ProducedTask, *, producer_key: str | None = None
    ) -> ProcessingResult:
        """Adopt a completed computation without submitting another model request.

        Output publication precedes the write transaction. If adoption fails,
        the error carries completed-stage and published-output details plus the
        ``ProducedTask`` as recovery context for the caller.

        Returns:
            The committed processing result and its output references.

        Raises:
            NousError: If publication or adoption fails. The exception includes
                the completed computation as recovery context.
        """
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
                    producer_key=producer_key,
                )
            record_completed(result.ref.identifier)
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
        producer_key: str | None = None,
        frozen_task_ref: ArtifactRef | None = None,
        request_identity: str | None = None,
    ) -> ProducedTask:
        async with self.execution.operation("model.generate", prepared.snapshot) as active:
            producer_key = producer_key or "generation:" + str(uuid4())
            response_key = producer_key + ":response"
            identity = (
                request_identity
                or hashlib.sha256(
                    json.dumps(
                        {
                            "task": prepared.task,
                            "prompt": prepared.compiled.resource.execution_signature,
                            "reading": reading_value(prepared.compiled.reading),
                        },
                        sort_keys=True,
                        ensure_ascii=False,
                    ).encode()
                ).hexdigest()
            )
            accepted = await self.results.find_producer(response_key)
            if accepted is not None:
                if accepted.parameters.get("request_identity") != identity:
                    raise NousError(
                        "content.producer_conflict",
                        "Saved response belongs to a different language request",
                        details={"producer_key": response_key},
                    )
                envelope = next(
                    output.artifact for output in accepted.outputs if output.name == "computation"
                )
                generated = read_generated(
                    json.loads(await descriptor_text(self.results, envelope))
                )
            else:
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
                reading = generated.reading or prepared.compiled.reading
                accepted = await self.results.adopt(
                    operation="model.language_response",
                    producer=generated.producer,
                    inputs=tuple(source.group for source in reading.sources if source.group),
                    outputs={
                        "response": OutputContent(
                            data=json.dumps(generated.response, ensure_ascii=False).encode(),
                            media_type="application/json",
                            display_name="response.json",
                        ),
                        "computation": OutputContent(
                            data=json.dumps(
                                generated_value(generated, reading), ensure_ascii=False
                            ).encode(),
                            media_type="application/json",
                            display_name="response-computation.json",
                        ),
                    },
                    request=generated.request,
                    parameters={
                        "request_identity": identity,
                        "producer_key": response_key,
                        "frozen_task_ref": frozen_task_ref.model_dump(mode="json")
                        if frozen_task_ref
                        else None,
                    },
                    coverage=generated.coverage,
                    producer_key=response_key,
                )
            return await self._interpret_response(
                prepared, generated, accepted, response_key, inputs, coverage
            )

    async def recover_response(
        self,
        ref: str | ResultRef,
        snapshot: ConfigSnapshot,
        *,
        frozen: FrozenTask | None = None,
    ) -> ProducedTask:
        """Interpret an accepted language response without sending another protocol request.

        A retained frozen task is loaded against the current resource bindings.
        Ordinary responses use the installed task with the same output contract.
        """
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
            retained = accepted.parameters.get("frozen_task_ref")
            if frozen is None:
                frozen = (
                    await self.load_language_task(ArtifactRef.model_validate(retained), snapshot)
                    if retained
                    else self.executor.freeze(str(generated.request["task"]), snapshot)
                )
            if (
                frozen.resource is None
                or frozen.resource.manifest.contract_id != generated.request.get("contract_id")
            ):
                raise NousError(
                    "model.task_contract_changed",
                    "Installed task contract differs from the accepted response",
                )
            assert generated.reading is not None
            schema = cast(dict[str, JsonValue] | None, generated.request.get("schema"))
            prepared = self.executor.prepare(
                frozen,
                generated.reading,
                snapshot,
                schema=schema if frozen.resource.manifest.contract_id == "custom-reading" else None,
            )
            if prepared.compiled.schema != schema:
                raise NousError(
                    "model.task_contract_changed",
                    "Installed output schema differs from the accepted response",
                )
            return await self._interpret_response(
                prepared,
                generated,
                accepted,
                str(accepted.parameters.get("producer_key") or accepted.ref.identifier),
                None,
                generated.coverage,
            )

    async def _interpret_response(
        self,
        prepared: PreparedGeneration,
        generated: GenerationResult,
        accepted: ProcessingResult,
        producer_key: str,
        inputs: Sequence[InputGroup] | None,
        coverage: Mapping[str, object] | None,
    ) -> ProducedTask:
        try:
            produced = await self._interpret(prepared, generated, inputs, coverage)
            return replace(
                produced,
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
                "program_outputs": list(definition.program_outputs),
                "representations": {"output": {"kind": definition.source_representation}}
                if definition.source_representation is not None
                else {},
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
                            kind="artifact",
                            artifact=artifact.ref,
                            selection=selected,
                            role=role,
                            slot_key=slot_key,
                        )
                        for artifact, selected, role, slot_key in originals
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
                    snapshot.values.media,
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
                        self.preparer, group, snapshot.values.media.max_inline_bytes
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
        settings = snapshot.values
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
