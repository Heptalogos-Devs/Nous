"""Normalize and prepare selected generation inputs for a model route."""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import replace
from typing import TYPE_CHECKING, Literal, cast

from pydantic import JsonValue

from nous.common.errors import NousError
from nous.content.contracts import (
    ContentSelection,
    ResultRef,
    SelectedInputs,
    SelectedSource,
    SourceFacts,
    SourceTarget,
)
from nous.content.results import ResultStore
from nous.models.configuration import ModelRoute
from nous.runtime.execution import ExecutionManager

from ..contracts import ReadingInput
from ..input import RoutePlan, SemanticUnit, for_route, materialize, plan_inputs
from ..policy import route_unavailable
from ..requests import FrozenTask, PreparedGeneration
from .contracts import (
    PendingGeneration,
    PreparedRun,
)
from .transcription import TranscriptionOperations

if TYPE_CHECKING:
    from nous.application.contracts import CallContext
    from nous.configuration import ConfigSnapshot
    from nous.media.preparation import MediaPreparer

    from ..executor import ModelExecutor


class GenerationPreparation:
    """Normalize selected inputs and prepare one eligible model route."""

    def __init__(
        self,
        executor: ModelExecutor,
        results: ResultStore,
        execution: ExecutionManager,
        preparer: MediaPreparer,
        transcription: TranscriptionOperations,
    ) -> None:
        self.executor, self.results, self.execution, self.preparer = (
            executor,
            results,
            execution,
            preparer,
        )
        self.transcription = transcription

    async def accept_targets(
        self, targets: Sequence[SourceTarget], call: CallContext
    ) -> SelectedInputs:
        """Accept caller bindings once and capture exact source slots."""
        sources: list[SelectedSource] = []
        for index, target in enumerate(targets, 1):
            artifact, selection = await call.inputs.accept_target(target)
            sources.append(
                SelectedSource(
                    slot_key=f"S{index}",
                    selections=(ContentSelection(artifact=artifact.ref, selection=selection),),
                    display_name=artifact.display_name,
                )
            )
        return await self._inspect_selected(SelectedInputs(sources=tuple(sources)), call.settings)

    async def _inspect_selected(
        self, selected: SelectedInputs, snapshot: ConfigSnapshot
    ) -> SelectedInputs:
        existing = {fact.artifact.id: fact for fact in selected.facts}
        references = tuple(
            dict.fromkeys(
                item.artifact for source in selected.sources for item in source.selections
            )
        )
        missing = tuple(ref for ref in references if ref.id not in existing)
        for artifact in await self.results.artifacts.get_many(missing):
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

    async def prepare_input(
        self,
        task: str | FrozenTask,
        input: SelectedInputs,
        snapshot: ConfigSnapshot,
        *,
        document_mode: str = "auto",
        without_audio: bool = False,
        input_mode: Literal["joint", "independent"] = "joint",
        unit: SemanticUnit | None = None,
    ) -> PreparedRun:
        async with self.execution.operation("model.prepare_input", snapshot):
            frozen = self.executor.freeze(task, snapshot) if isinstance(task, str) else task
            selected_inputs = await self._inspect_selected(input, snapshot)
            if unit is not None:
                unit = replace(unit, inputs=selected_inputs)
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
                        input_mode=input_mode,
                        unit=unit,
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
                frozen,
                plan,
                tuple(
                    dict.fromkeys(
                        ref
                        for item in plan.representation_plans
                        for ref in item.completed_result_refs
                    )
                ),
            )

    async def prepare_group(
        self,
        frozen: FrozenTask,
        group: RoutePlan,
        snapshot: ConfigSnapshot,
        *,
        question: str = "",
        instructions: str = "",
        schema: dict[str, JsonValue] | None = None,
        preview: bool = False,
        streaming: bool = False,
        route: ModelRoute | None = None,
        completed: list[ResultRef] | None = None,
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
                    planned = (
                        group
                        if route is None and candidate == candidates[0]
                        else await for_route(
                            self.preparer,
                            group,
                            candidate.model.input_formats,
                            (frozen.media_settings or snapshot.values.media),
                        )
                    )
                    batch = await materialize(
                        self.preparer,
                        planned,
                        min(
                            (frozen.media_settings or snapshot.values.media).max_inline_bytes,
                            snapshot.values.media.max_inline_bytes,
                        ),
                    )
                    completed.extend(batch.completed_result_refs)
                    reading = ReadingInput(question, instructions, batch.sources)
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
                    reading = await self.transcription.language_sources(
                        reading, selected, snapshot, context, completed
                    )
                    prepared = replace(
                        self.executor.prepare(selected, reading, snapshot, schema=schema),
                        stage_refs=tuple(dict.fromkeys(completed)),
                        coverage=batch.coverage,
                    )
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
