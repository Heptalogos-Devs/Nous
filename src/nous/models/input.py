"""Semantic source ordering and request grouping for every language entry point."""

from __future__ import annotations

from collections import Counter
from collections.abc import Sequence
from dataclasses import dataclass, replace
from typing import TYPE_CHECKING

from pydantic import JsonValue

from nous.common.errors import NousError
from nous.content.contracts import (
    Artifact,
    ContentSelection,
    SelectedInputs,
    SelectedSource,
    SourceRole,
    SourceSelection,
)
from nous.media.contracts import MediaPlan, PreparedBatch

from .tasks import TaskDefinition

if TYPE_CHECKING:
    from nous.media.configuration import MediaConfig
    from nous.media.preparation import MediaPreparer


@dataclass(frozen=True)
class RequestGroup:
    batches: tuple[PreparedBatch, ...]
    coverage: dict[str, JsonValue]
    selected_inputs: SelectedInputs
    route_inputs: SelectedInputs
    contract: TaskDefinition | None = None
    document_mode: str = "auto"
    without_audio: bool = False


@dataclass(frozen=True)
class InputPlan:
    task_mode: str
    selected_inputs: SelectedInputs
    representation_plans: tuple[MediaPlan, ...]
    request_groups: tuple[RequestGroup, ...]
    requested_coverage: dict[str, JsonValue]


async def plan_inputs(
    preparer: MediaPreparer,
    selected_inputs: SelectedInputs,
    settings: MediaConfig,
    *,
    requirements: Sequence[str] = (),
    document_mode: str = "auto",
    without_audio: bool = False,
    per_batch: bool = False,
    contract: TaskDefinition | None = None,
) -> InputPlan:
    mode = "independent_batches" if per_batch else contract.input_mode if contract else "joint"
    originals: list[tuple[Artifact, SourceSelection, SourceRole, str, str | None]] = []
    for source in selected_inputs.sources:
        artifacts = await preparer.artifacts.get_many([item.artifact for item in source.selections])
        originals.extend(
            (
                artifact.model_copy(update={"display_name": source.display_name})
                if source.display_name is not None
                else artifact,
                item.selection,
                source.role,
                source.slot_key,
                source.display_name,
            )
            for item, artifact in zip(source.selections, artifacts, strict=True)
        )
    facts = {item.artifact.id: item.details for item in selected_inputs.facts}
    plans: list[MediaPlan] = []
    batches: list[PreparedBatch] = []
    chosen_inputs = tuple((artifact, supplied) for artifact, supplied, _, _, _ in originals)
    task_plans = (
        await contract.prepare_sources(chosen_inputs, preparer.artifacts) if contract else None
    )
    slots = [slot for _, _, _, slot, _ in originals]
    for declared in slots:
        if any(char.isspace() for char in declared):
            raise NousError(
                "model.invalid_source_slot", "Source slot keys cannot contain whitespace"
            )
    roles: dict[str, SourceRole] = {}
    for slot, (_, _, role, _, _) in zip(slots, originals, strict=True):
        if slot in roles and roles[slot] != role:
            raise NousError(
                "model.source_slot_conflict",
                "One source slot cannot have different roles",
                details={"slot_key": slot},
            )
        roles[slot] = role
    counts = Counter(slots)
    fragments: Counter[str] = Counter()
    seen_keys: set[str] = set()
    route_batches: list[SelectedInputs] = []
    for index, (artifact, supplied, role, _, name) in enumerate(originals, 1):
        slot_key = slots[index - 1]
        fragments[slot_key] += 1
        chosen = supplied
        if task_plans is not None:
            plan = task_plans[index - 1]
        else:
            plan = await preparer.prepare(
                artifact,
                chosen,
                requirements=requirements,
                settings=settings,
                document_mode=document_mode,
                without_audio=without_audio,
                facts=facts.get(artifact.ref.id),
            )
        plans.append(plan)
        for batch in plan.batches:
            prefix = slot_key if counts[slot_key] == 1 else f"{slot_key}:F{fragments[slot_key]}"
            preserve = contract is not None and contract.preserve_source_keys
            sources = tuple(
                source.bind_slot(slot_key, prefix, role, preserve_key=preserve)
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
            batch = replace(
                batch,
                sources=sources,
                source_prefix=prefix,
                source_slot_key=slot_key,
                source_role=role,
            )
            batches.append(batch)
            route_batches.append(
                SelectedInputs(
                    sources=(
                        SelectedSource(
                            slot_key=slot_key,
                            role=role,
                            selections=(
                                ContentSelection(artifact=artifact.ref, selection=batch.selection),
                            ),
                            display_name=name,
                        ),
                    ),
                    facts=selected_inputs.facts,
                )
            )
    coverage: dict[str, JsonValue] = {
        "mode": mode,
        "inputs": [
            {"artifact": plan.artifact.ref.model_dump(mode="json"), **plan.coverage}
            for plan in plans
        ],
    }
    requests = (
        tuple(
            RequestGroup(
                (batch,),
                batch.coverage,
                selected_inputs,
                route_inputs,
                contract,
                document_mode,
                without_audio,
            )
            for batch, route_inputs in zip(batches, route_batches, strict=True)
        )
        if mode == "independent_batches"
        else (
            RequestGroup(
                tuple(batches),
                coverage,
                selected_inputs,
                selected_inputs,
                contract,
                document_mode,
                without_audio,
            ),
        )
    )
    return InputPlan(mode, selected_inputs, tuple(plans), requests, coverage)


async def materialize(
    preparer: MediaPreparer, request: RequestGroup, max_bytes: int
) -> PreparedBatch:
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
                "Joint input exceeds the request byte allowance; reduce the selected range or explicitly use per-batch processing",
                details={
                    "bytes": total,
                    "max_bytes": max_bytes,
                    "coverage": request.coverage,
                    "completed_result_refs": [
                        ref for item in (*batches, batch) for ref in item.completed_result_refs
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
        {**request.coverage, "actual": [batch.coverage for batch in batches]}
        if request.coverage.get("mode") == "joint" or len(batches) != 1
        else batches[0].coverage,
        completed_result_refs=tuple(
            dict.fromkeys(ref for batch in batches for ref in batch.completed_result_refs)
        ),
    )


async def for_route(
    preparer: MediaPreparer,
    request: RequestGroup,
    requirements: Sequence[str],
    settings: MediaConfig,
) -> RequestGroup:
    """Replan the selected physical range from the retained original selections."""
    plan = await plan_inputs(
        preparer,
        request.route_inputs,
        settings,
        requirements=requirements,
        document_mode=request.document_mode,
        without_audio=request.without_audio,
        contract=request.contract,
    )
    batches = tuple(batch for group in plan.request_groups for batch in group.batches)
    coverage = (
        plan.requested_coverage
        if len(batches) != 1 or request.coverage.get("mode") == "joint"
        else batches[0].coverage
    )
    return replace(request, batches=batches, coverage=coverage)
