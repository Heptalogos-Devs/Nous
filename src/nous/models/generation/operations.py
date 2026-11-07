"""Complete generation, template preview and model probes over shared content plans."""

from __future__ import annotations

import asyncio
import hashlib
import json
from collections.abc import Awaitable, Callable, Mapping, Sequence
from dataclasses import dataclass, replace
from pathlib import Path
from typing import TYPE_CHECKING, Literal, cast
from uuid import uuid4
from weakref import WeakValueDictionary

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
    OutputMapping,
    ProcessingResult,
    ResultRef,
    SelectedInputs,
    SelectedSource,
    SourceFacts,
    SourceRole,
    SourceSelection,
)
from nous.content.results import PreparedOutputs, ResultStore
from nous.media.contracts import PreparedSource
from nous.models.configuration import ModelRoute, PresetConfig, TaskAssignment
from nous.runtime.execution import ExecutionManager, OperationContext

from ..contracts import GenerationResult, ReadingInput
from ..executor import FrozenTask, PreparedGeneration, UnavailableTask
from ..input import InputPlan, RequestGroup, for_route, materialize, plan_inputs
from ..policy import route_unavailable
from ..tasks import schema_semantics
from .computation import (
    FrozenLanguageTask,
    descriptor_text,
    freeze_task,
    generated_value,
    load_task,
    read_generated,
    reading_value,
)

if TYPE_CHECKING:
    from nous.configuration import ConfigSnapshot
    from nous.media.preparation import MediaPreparer
    from nous.storage.database import WriteTx

    from ..executor import ModelExecutor

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
    selected_inputs: SelectedInputs | None = None
    dependencies: tuple[ArtifactRef | ResultRef, ...] = ()


@dataclass(frozen=True)
class GenerationRequest:
    task: str | FrozenLanguageTask
    inputs: SelectedInputs
    question: str = ""
    instructions: str = ""
    schema: dict[str, JsonValue] | None = None
    mode: Literal["joint", "independent_batches"] = "joint"
    dependencies: tuple[ArtifactRef | ResultRef, ...] = ()
    document_mode: str = "auto"
    without_audio: bool = False

    def identity(self) -> str:
        return hashlib.sha256(
            json.dumps(
                {
                    "task": self.task
                    if isinstance(self.task, str)
                    else self.task.model_dump(mode="json"),
                    "inputs": self.inputs.identity(),
                    "question": self.question,
                    "instructions": self.instructions,
                    "schema": schema_semantics(self.schema) if self.schema is not None else None,
                    "mode": self.mode,
                    "dependencies": [ref.model_dump(mode="json") for ref in self.dependencies],
                    "document_mode": self.document_mode,
                    "without_audio": self.without_audio,
                },
                sort_keys=True,
                ensure_ascii=False,
            ).encode()
        ).hexdigest()


@dataclass(frozen=True)
class PreparedProduction:
    """Published outputs and their adoption facts, reusable after a transaction rollback."""

    operation: str
    producer: dict[str, JsonValue]
    actual_inputs: tuple[InputGroup, ...]
    outputs: PreparedOutputs
    request: dict[str, JsonValue]
    parameters: dict[str, JsonValue]
    coverage: dict[str, JsonValue]
    producer_key: str | None
    completed_stage_refs: tuple[str, ...]
    dependencies: tuple[ArtifactRef | ResultRef, ...]
    mappings: tuple[OutputMapping, ...]


@dataclass(frozen=True)
class PendingResponse:
    """A successful response and its already published bytes awaiting adoption."""

    generated: GenerationResult
    outputs: PreparedOutputs
    inputs: tuple[InputGroup, ...]
    identity: str
    producer_key: str
    frozen_task_ref: ArtifactRef | None
    selected_inputs: SelectedInputs | None
    dependencies: tuple[ArtifactRef | ResultRef, ...]
    interpretation_contract: dict[str, JsonValue]


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
        self._computation_locks: WeakValueDictionary[str, asyncio.Lock] = WeakValueDictionary()
        self._result_locks: WeakValueDictionary[str, asyncio.Lock] = WeakValueDictionary()

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

    def freeze_transcription(self, snapshot: ConfigSnapshot) -> FrozenTask | None:
        task = snapshot.values.media.audio.transcription_task
        return self.executor.freeze(task, snapshot) if task else None

    async def _transcription_for_route(
        self, frozen: FrozenTask, snapshot: ConfigSnapshot
    ) -> FrozenTask:
        """Resolve the frozen prerequisite only when this route needs audio text."""
        prerequisite = frozen.transcription
        task = (
            frozen.media_settings.audio.transcription_task
            if frozen.media_settings is not None
            else snapshot.values.media.audio.transcription_task
        )
        if isinstance(prerequisite, UnavailableTask):
            raise NousError(
                "media.audio_capability_missing",
                "The selected route requires an unavailable transcription task",
                details={
                    "transcription_task": prerequisite.task,
                    "configuration_error": {
                        "code": prerequisite.code,
                        "message": prerequisite.message,
                        "details": prerequisite.details,
                        "retryable": prerequisite.retryable,
                    },
                },
            )
        try:
            if isinstance(prerequisite, ArtifactRef):
                return await self.load_language_task(prerequisite, snapshot)
            if frozen.media_settings is None:
                current = self.freeze_transcription(snapshot)
                if current is not None:
                    return current
        except NousError as error:
            if error.code not in {
                "model.task_unconfigured",
                "model.resource_binding",
                "model.task_contract_changed",
            } and not error.code.startswith("prompt."):
                raise
            raise NousError(
                "media.audio_capability_missing",
                "The selected route requires an unavailable transcription task",
                details={"transcription_task": task, "configuration_error": error.to_dict()},
                cause=error,
            ) from error
        raise NousError(
            "media.audio_capability_missing",
            "The selected route requires a configured transcription task",
            details={"transcription_task": task},
        )

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

    async def accept_inputs(
        self,
        inputs: InputValue | Sequence[InputValue],
        snapshot: ConfigSnapshot,
        *,
        texts: Sequence[str] = (),
    ) -> SelectedInputs:
        """Accept and normalize original selections without deriving physical representations."""
        originals = await self._accept(inputs, texts)
        explicit = {slot for _, _, _, slot in originals if slot is not None}
        grouped: dict[str, tuple[SourceRole, list[ContentSelection], str | None]] = {}
        counter = 1
        for artifact, selection, role, slot in originals:
            if slot is None:
                while f"S{counter}" in explicit or f"S{counter}" in grouped:
                    counter += 1
                slot = f"S{counter}"
                counter += 1
            if any(char.isspace() for char in slot):
                raise NousError(
                    "model.invalid_source_slot", "Source slot keys cannot contain whitespace"
                )
            if slot in grouped and grouped[slot][0] != role:
                raise NousError(
                    "model.source_slot_conflict",
                    "One source slot cannot have different roles",
                    details={"slot_key": slot},
                )
            if slot not in grouped:
                grouped[slot] = (role, [], artifact.display_name)
            grouped[slot][1].append(ContentSelection(artifact=artifact.ref, selection=selection))
        values = [selection for _, selections, _ in grouped.values() for selection in selections]
        inspected: dict[str, dict[str, JsonValue]] = {}
        facts: list[SourceFacts] = []
        for artifact, _, _, _ in originals:
            if artifact.ref.id in inspected:
                continue
            info = await self.preparer.inspect(artifact, snapshot.values.media)
            inspected[artifact.ref.id] = info
            facts.append(
                SourceFacts(
                    artifact=artifact.ref,
                    declared_mime=artifact.media_type,
                    detected_mime=str(info["media_type"]),
                    kind=cast(Literal["text", "image", "pdf", "audio", "video"], info["kind"]),
                    details=info,
                )
            )
        normalized = await self.preparer.validate_selections(
            values, settings=snapshot.values.media, facts=inspected
        )
        sources: list[SelectedSource] = []
        offset = 0
        for slot, (role, selections, name) in grouped.items():
            sources.append(
                SelectedSource(
                    slot_key=slot,
                    role=role,
                    selections=normalized[offset : offset + len(selections)],
                    display_name=name,
                )
            )
            offset += len(selections)
        return SelectedInputs(sources=tuple(sources), facts=tuple(facts))

    async def _inspect_selected(
        self, selected: SelectedInputs, snapshot: ConfigSnapshot
    ) -> SelectedInputs:
        existing = {fact.artifact.id: fact for fact in selected.facts}
        references = tuple(
            dict.fromkeys(
                item.artifact for source in selected.sources for item in source.selections
            )
        )
        if all(ref.id in existing for ref in references):
            return selected
        for artifact in await self.results.artifacts.get_many(references):
            if artifact.ref.id in existing:
                continue
            info = await self.preparer.inspect(artifact, snapshot.values.media)
            existing[artifact.ref.id] = SourceFacts(
                artifact=artifact.ref,
                declared_mime=artifact.media_type,
                detected_mime=str(info["media_type"]),
                kind=cast(Literal["text", "image", "pdf", "audio", "video"], info["kind"]),
                details=info,
            )
        normalized = await self.preparer.validate_selections(
            [item for source in selected.sources for item in source.selections],
            settings=snapshot.values.media,
            facts={key: fact.details for key, fact in existing.items()},
        )
        sources: list[SelectedSource] = []
        offset = 0
        for source in selected.sources:
            count = len(source.selections)
            sources.append(
                source.model_copy(update={"selections": normalized[offset : offset + count]})
            )
            offset += count
        return SelectedInputs(sources=tuple(sources), facts=tuple(existing.values()))

    async def produce(
        self,
        request: GenerationRequest,
        *,
        computation_key: str,
        snapshot: ConfigSnapshot,
        on_delta: Callable[[str], None] | None = None,
    ) -> ProducedTask:
        """Restore an accepted response or perform one semantic language computation."""
        identity = request.identity()
        lock = self._computation_locks.setdefault(computation_key, asyncio.Lock())
        async with lock:
            accepted = await self.results.find_producer(computation_key + ":response")
            if accepted is not None:
                if accepted.parameters.get("request_identity") != identity:
                    raise NousError(
                        "content.producer_conflict",
                        "Saved response belongs to a different language request",
                        details={"producer_key": computation_key},
                    )
                return await self.recover_response(accepted.ref, snapshot)
            frozen = (
                await self.load_language_task(request.task, snapshot)
                if isinstance(request.task, FrozenLanguageTask)
                else self.executor.freeze(request.task, snapshot)
            )
            prepared = await self.prepare_input(
                frozen,
                request.inputs,
                snapshot,
                question=request.question,
                instructions=request.instructions,
                schema=request.schema,
                document_mode=request.document_mode,
                without_audio=request.without_audio,
                per_batch=request.mode == "independent_batches",
            )
            if len(prepared.batches) != 1:
                raise NousError(
                    "model.single_computation_required",
                    "This computation requires one semantic request group",
                )
            assert prepared.input_plan is not None
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
            return await self._produce_group(
                prepared.frozen,
                prepared.batches[0],
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
            )

    async def prepare_input(
        self,
        task: str | FrozenTask,
        input: SelectedInputs | InputValue | Sequence[InputValue],
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
            if isinstance(input, SelectedInputs):
                selected_inputs = await self._inspect_selected(input, snapshot)
            else:
                selected_inputs = await self.accept_inputs(input, snapshot, texts=texts)
            if selection is not None:
                selected_inputs = selected_inputs.model_copy(
                    update={
                        "sources": tuple(
                            source.model_copy(
                                update={
                                    "selections": tuple(
                                        item.model_copy(
                                            update={"selection": item.selection.restrict(selection)}
                                        )
                                        for item in source.selections
                                    )
                                }
                            )
                            for source in selected_inputs.sources
                        )
                    }
                )
                normalized = await self.preparer.validate_selections(
                    [item for source in selected_inputs.sources for item in source.selections],
                    settings=snapshot.values.media,
                    facts={fact.artifact.id: fact.details for fact in selected_inputs.facts},
                )
                offset = 0
                sources: list[SelectedSource] = []
                for source in selected_inputs.sources:
                    count = len(source.selections)
                    sources.append(
                        source.model_copy(
                            update={"selections": normalized[offset : offset + count]}
                        )
                    )
                    offset += count
                selected_inputs = selected_inputs.model_copy(update={"sources": tuple(sources)})
            unavailable: dict[str, JsonValue] = {}
            routes = frozen.routes
            if any(fact.kind == "video" for fact in selected_inputs.facts):
                # Prefer an explicitly declared native video route over sampled frames.
                routes = tuple(
                    sorted(
                        routes,
                        key=lambda item: (
                            not (
                                "video" in item.model.capabilities
                                and any(
                                    mime.startswith("video/") for mime in item.model.input_formats
                                )
                            )
                        ),
                    )
                )
            for route_index, route in enumerate(routes):
                try:
                    self.executor.validate_route(frozen, route, snapshot)
                    plan = await plan_inputs(
                        self.preparer,
                        selected_inputs,
                        (frozen.media_settings or snapshot.values.media),
                        requirements=route.model.input_formats,
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
                        frozen, routes=routes[route_index:], ineligible_routes=unavailable
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
                tuple(
                    await self.results.artifacts.get_many(
                        [
                            item.artifact
                            for source in selected_inputs.sources
                            for item in source.selections
                        ]
                    )
                ),
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
        parameters: Mapping[str, JsonValue] | None = None,
        dependencies: Sequence[ArtifactRef | ResultRef] = (),
        frozen_task_ref: ArtifactRef | None = None,
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
            produced = await self._produce_group(
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
                dependencies=dependencies,
                frozen_task_ref=frozen_task_ref,
            )
            if parameters:
                produced = replace(produced, parameters={**produced.parameters, **parameters})
            return await self._adopt(produced)

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
        async with self.execution.operation("model.generate", snapshot):
            selected_inputs = selected_inputs or group.selected_inputs
            identity = (
                request_identity
                or hashlib.sha256(
                    json.dumps(
                        {
                            "task": frozen.task,
                            "prompt": frozen.resource.execution_signature
                            if frozen.resource
                            else None,
                            "question": question,
                            "instructions": instructions,
                            "schema": schema,
                            "inputs": selected_inputs.identity(),
                            "range": group.route_inputs.identity(),
                            "extra_sources": reading_value(
                                ReadingInput(sources=tuple(extra_sources))
                            ),
                        },
                        sort_keys=True,
                        ensure_ascii=False,
                    ).encode()
                ).hexdigest()
            )
            if producer_key is not None:
                accepted = await self.results.find_producer(producer_key + ":response")
                if accepted is not None:
                    if accepted.parameters.get("request_identity") != identity:
                        raise NousError(
                            "content.producer_conflict",
                            "Saved response belongs to a different language request",
                            details={"producer_key": producer_key},
                        )
                    return await self.recover_response(accepted.ref, snapshot)

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
                selected_inputs=selected_inputs,
                dependencies=dependencies,
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
        frozen = frozen or self.executor.freeze(task, snapshot)
        identity = hashlib.sha256(
            json.dumps(
                {
                    "task": task,
                    "input": part.model_dump(mode="json"),
                    "assignment": frozen.assignment.model_dump(mode="json"),
                    "routes": [
                        {
                            "id": route.model_id,
                            "model": route.model.model_dump(mode="json"),
                            "preset": route.preset.model_dump(mode="json"),
                        }
                        for route in frozen.routes
                    ],
                },
                sort_keys=True,
                ensure_ascii=False,
            ).encode()
        ).hexdigest()
        producer_key = "transcription:" + identity
        lock = self._computation_locks.setdefault(producer_key, asyncio.Lock())
        async with lock:
            accepted = await self.results.find_producer(producer_key)
            if accepted is not None:
                return accepted
            artifact = await self.results.artifacts.get(part.content.artifact)
            generated = await self.executor.transcribe(
                frozen,
                await self.results.artifacts.read_bytes(artifact.ref),
                artifact.display_name,
                snapshot,
                context=context,
            )
            if not isinstance(generated.output, str):
                raise NousError(
                    "media.invalid_transcript", "Transcription returned a non-text result"
                )
            data = generated.output.encode()
            origins = tuple(item.origin for item in part.origins) or (part.content,)
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
                        data=data, media_type="text/plain", display_name="transcript.txt"
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
                mappings=(
                    OutputMapping(
                        output_name="transcript",
                        selection=SourceSelection(start_byte=0, end_byte=len(data)),
                        relation="atomic",
                        origins=origins,
                    ),
                )
                if data
                else (),
                producer_key=producer_key,
            )

    async def language_sources(
        self,
        reading: ReadingInput,
        frozen: FrozenTask,
        snapshot: ConfigSnapshot,
        context: OperationContext,
        completed: list[str],
    ) -> ReadingInput:
        """Resolve language-dependent representations after deterministic preparation has finished."""
        sources: list[PreparedSource] = []
        transcription: FrozenTask | None = None
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
                if transcription is None:
                    transcription = await self._transcription_for_route(frozen, snapshot)
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
        request: GenerationRequest,
        snapshot: ConfigSnapshot,
        *,
        computation_key: str | None = None,
    ) -> ProcessingResult:
        key = computation_key or "generation:" + str(uuid4())
        lock = self._result_locks.setdefault(key, asyncio.Lock())
        async with lock:
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
            return await self._adopt(
                await self.produce_prepared(
                    prepared,
                    inputs=inputs,
                    coverage=coverage,
                    context=context,
                    on_delta=on_delta,
                    prepare_route=prepare_route,
                )
            )

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
            produced.completed_stage_refs,
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
        selected_inputs: SelectedInputs | None = None,
        dependencies: Sequence[ArtifactRef | ResultRef] = (),
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
            pending = None
            try:
                published = await self.results.prepare_outputs(
                    {
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
                    }
                )
                pending = PendingResponse(
                    generated,
                    published,
                    tuple(source.group for source in reading.sources if source.group),
                    identity,
                    response_key,
                    frozen_task_ref,
                    selected_inputs,
                    tuple(dependencies),
                    self.executor.compiler.catalog.tasks[
                        str(generated.request["contract_id"])
                    ].interpretation_contract(),
                )
                accepted = await self._adopt_response(pending)
            except BaseException as error:
                add_error_context(
                    error,
                    {"stage": "model.response_adopt", "model_succeeded": True},
                    recovery=pending or generated,
                )
                raise
            return await self._interpret_response(
                generated, accepted, response_key, inputs, coverage
            )

    async def _adopt_response(self, pending: PendingResponse) -> ProcessingResult:
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
                },
                coverage=pending.generated.coverage,
                producer_key=pending.producer_key,
                dependencies=pending.dependencies,
            )
        record_completed(result.ref.identifier)
        return result

    async def resume_response(
        self, pending: PendingResponse, snapshot: ConfigSnapshot
    ) -> ProducedTask:
        """Finish adopting an in-memory successful response without publishing it again."""
        async with self.execution.operation("model.resume_response", snapshot):
            accepted = await self._adopt_response(pending)
            return await self._interpret_response(
                pending.generated, accepted, pending.producer_key, None, pending.generated.coverage
            )

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
            contract_id = str(generated.request.get("contract_id", ""))
            tasks = self.executor.compiler.catalog.tasks
            if contract_id not in tasks:
                raise NousError(
                    "model.task_contract_changed",
                    "The task interpreter for this response is not installed",
                    details={"contract_id": contract_id},
                )
            definition = tasks[contract_id]
            if definition.interpretation_contract() != accepted.parameters.get(
                "interpretation_contract"
            ):
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
            return await self._interpret_response(
                generated,
                accepted,
                str(accepted.parameters.get("producer_key") or accepted.ref.identifier),
                None,
                generated.coverage,
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
            selected = (
                await self.accept_inputs(input, snapshot, texts=texts)
                if input or texts
                else SelectedInputs(sources=())
            )
            if frozen and selected.sources:
                prepared = await self.prepare_input(
                    frozen,
                    selected,
                    snapshot,
                    per_batch=per_batch,
                )
                assert prepared.input_plan is not None
                plan, frozen = prepared.input_plan, prepared.frozen
            else:
                plan = await plan_inputs(
                    self.preparer,
                    selected,
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
                            language=snapshot.values.localization.language,
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
