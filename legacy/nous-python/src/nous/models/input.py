"""Choose semantic units before planning their physical model representations."""

from __future__ import annotations

from collections import Counter
from collections.abc import Sequence
from dataclasses import dataclass, replace
from typing import TYPE_CHECKING, Literal

from pydantic import JsonValue

from nous.common.errors import NousError
from nous.content.contracts import (
    Artifact,
    SelectedInputs,
    SourceRole,
    SourceSelection,
)
from nous.media.contracts import MediaPlan, PreparedBatch

from .tasks import TaskDefinition

if TYPE_CHECKING:
    from nous.media.configuration import MediaConfig
    from nous.media.preparation import MediaPreparer


@dataclass(frozen=True)
class SemanticUnit:
    key: str
    inputs: SelectedInputs
    mode: Literal["joint", "independent"]


@dataclass(frozen=True)
class RoutePlan:
    unit: SemanticUnit
    batches: tuple[PreparedBatch, ...]
    coverage: dict[str, JsonValue]
    contract: TaskDefinition | None = None
    document_mode: str = "auto"
    without_audio: bool = False


@dataclass(frozen=True)
class InputPlan:
    task_mode: str
    selected_inputs: SelectedInputs
    representation_plans: tuple[MediaPlan, ...]
    route_plans: tuple[RoutePlan, ...]
    requested_coverage: dict[str, JsonValue]


def semantic_units(
    selected: SelectedInputs,
    mode: Literal["joint", "independent"],
    ranges: Sequence[SourceSelection] = (),
    *,
    contract: TaskDefinition | None = None,
) -> tuple[SemanticUnit, ...]:
    """Partition selected material, retaining auxiliary context with each unit."""
    if mode == "joint":
        if ranges:
            raise NousError(
                "model.invalid_input_mode", "Independent ranges require independent input"
            )
        return (SemanticUnit("U1", selected, mode),)
    material = [
        (index, selection)
        for index, source in enumerate(selected.sources)
        if source.role != "context"
        for selection in source.selections
    ]
    if ranges:
        if len(material) != 1:
            raise NousError("model.invalid_input_mode", "Ranges require one selected source")
        index, original = material[0]
        material = [
            (index, original.model_copy(update={"selection": original.selection.restrict(item)}))
            for item in ranges
        ]
    elif contract is not None:
        had_material = bool(material)
        facts = {fact.artifact.id: fact for fact in selected.facts}
        material = [
            (index, original.model_copy(update={"selection": original.selection.restrict(item)}))
            for index, original in material
            for item in contract.independent_ranges(original, facts.get(original.artifact.id))
        ]
        if had_material and not material:
            raise NousError("model.empty_semantic_units", "Task produced no independent ranges")
    if not material:
        return (SemanticUnit("U1", selected, mode),)
    units: list[SemanticUnit] = []
    for index, selection in material:
        sources = tuple(
            source.model_copy(update={"selections": (selection,)})
            if source_index == index
            else source
            for source_index, source in enumerate(selected.sources)
            if source_index == index or source.role == "context"
        )
        refs = {item.artifact.id for source in sources for item in source.selections}
        inputs = SelectedInputs(
            sources=sources,
            facts=tuple(fact for fact in selected.facts if fact.artifact.id in refs),
        )
        units.append(SemanticUnit(f"U{len(units) + 1}", inputs, mode))
    return tuple(units)


async def _plan_unit(
    preparer: MediaPreparer,
    unit: SemanticUnit,
    settings: MediaConfig,
    *,
    requirements: Sequence[str],
    document_mode: str,
    without_audio: bool,
    contract: TaskDefinition | None,
) -> tuple[RoutePlan, tuple[MediaPlan, ...]]:
    originals: list[tuple[Artifact, SourceSelection, SourceRole, str]] = []
    for source in unit.inputs.sources:
        artifacts = await preparer.artifacts.get_many([item.artifact for item in source.selections])
        originals.extend(
            (
                artifact.model_copy(update={"display_name": source.display_name})
                if source.display_name is not None
                else artifact,
                item.selection,
                source.role,
                source.slot_key,
            )
            for item, artifact in zip(source.selections, artifacts, strict=True)
        )
    slots = [slot for _, _, _, slot in originals]
    if any(any(char.isspace() for char in slot) for slot in slots):
        raise NousError("model.invalid_source_slot", "Source slot keys cannot contain whitespace")
    roles: dict[str, SourceRole] = {}
    for _, _, role, slot in originals:
        if slot in roles and roles[slot] != role:
            raise NousError(
                "model.source_slot_conflict", "One source slot cannot have different roles"
            )
        roles[slot] = role
    facts = {item.artifact.id: item.details for item in unit.inputs.facts}
    chosen = tuple((artifact, selection) for artifact, selection, _, _ in originals)
    task_plans = await contract.prepare_sources(chosen, preparer.artifacts) if contract else None
    if task_plans is not None and len(task_plans) != len(originals):
        raise NousError("model.invalid_plan", "Task preparation must cover every selected input")
    counts, fragments = Counter(slots), Counter[str]()
    seen_keys: set[str] = set()
    plans: list[MediaPlan] = []
    batches: list[PreparedBatch] = []
    for index, (artifact, selection, role, slot) in enumerate(originals):
        fragments[slot] += 1
        plan = (
            task_plans[index]
            if task_plans is not None
            else await preparer.prepare(
                artifact,
                selection,
                requirements=requirements,
                settings=settings,
                document_mode=document_mode,
                without_audio=without_audio,
                facts=facts.get(artifact.ref.id),
            )
        )
        plans.append(plan)
        for batch in plan.batches:
            prefix = slot if counts[slot] == 1 else f"{slot}:F{fragments[slot]}"
            preserve = contract is not None and contract.preserve_source_keys
            sources = tuple(
                source.bind_slot(slot, prefix, role, preserve_key=preserve)
                for source in batch.sources
            )
            for source in sources:
                if source.key in seen_keys:
                    raise NousError(
                        "model.duplicate_source",
                        "Actual source group keys must be unique",
                        details={"key": source.key},
                    )
                seen_keys.add(source.key)
            batches.append(
                replace(
                    batch,
                    sources=sources,
                    source_prefix=prefix,
                    source_slot_key=slot,
                    source_role=role,
                )
            )
    coverage: dict[str, JsonValue] = {
        "mode": unit.mode,
        "unit": unit.key,
        "inputs": [
            {"artifact": plan.artifact.ref.model_dump(mode="json"), **plan.coverage}
            for plan in plans
        ],
    }
    return (
        RoutePlan(unit, tuple(batches), coverage, contract, document_mode, without_audio),
        tuple(plans),
    )


async def plan_inputs(
    preparer: MediaPreparer,
    selected_inputs: SelectedInputs,
    settings: MediaConfig,
    *,
    requirements: Sequence[str] = (),
    document_mode: str = "auto",
    without_audio: bool = False,
    input_mode: Literal["joint", "independent"] = "joint",
    contract: TaskDefinition | None = None,
    unit: SemanticUnit | None = None,
) -> InputPlan:
    mode: Literal["joint", "independent"] = (
        unit.mode
        if unit
        else (
            "independent"
            if input_mode == "independent" or (contract and contract.input_mode == "independent")
            else "joint"
        )
    )
    routes: list[RoutePlan] = []
    plans: list[MediaPlan] = []
    units = (unit,) if unit else semantic_units(selected_inputs, mode, contract=contract)
    for current in units:
        route, representations = await _plan_unit(
            preparer,
            current,
            settings,
            requirements=requirements,
            document_mode=document_mode,
            without_audio=without_audio,
            contract=contract,
        )
        routes.append(route)
        plans.extend(representations)
    coverage: dict[str, JsonValue] = {
        "mode": mode,
        "units": [route.coverage for route in routes],
    }
    return InputPlan(mode, selected_inputs, tuple(plans), tuple(routes), coverage)


async def materialize(preparer: MediaPreparer, request: RoutePlan, max_bytes: int) -> PreparedBatch:
    batches: list[PreparedBatch] = []
    total = 0
    for planned in request.batches:
        batch = await preparer.materialize(planned)
        total += sum(
            len((source.text or "").encode()) + sum(len(item.data) for item in source.media)
            for source in batch.sources
        )
        if total > max_bytes:
            raise NousError(
                "model.joint_input_limit",
                "The selected semantic unit exceeds the request byte allowance",
                details={
                    "bytes": total,
                    "max_bytes": max_bytes,
                    "coverage": request.coverage,
                    "completed_refs": [
                        ref.model_dump(mode="json")
                        for item in (*batches, batch)
                        for ref in item.completed_result_refs
                    ],
                },
            )
        batches.append(batch)
    keys = [source.key for batch in batches for source in batch.sources]
    if len(keys) != len(set(keys)):
        raise NousError("model.duplicate_source", "Materialized source group keys must be unique")
    return PreparedBatch(
        tuple(source for batch in batches for source in batch.sources),
        SourceSelection(),
        {**request.coverage, "actual": [batch.coverage for batch in batches]},
        completed_result_refs=tuple(
            dict.fromkeys(ref for batch in batches for ref in batch.completed_result_refs)
        ),
    )


async def for_route(
    preparer: MediaPreparer,
    request: RoutePlan,
    requirements: Sequence[str],
    settings: MediaConfig,
) -> RoutePlan:
    """Replan the same semantic unit for a different model route."""
    route, _ = await _plan_unit(
        preparer,
        request.unit,
        settings,
        requirements=requirements,
        document_mode=request.document_mode,
        without_audio=request.without_audio,
        contract=request.contract,
    )
    return route
