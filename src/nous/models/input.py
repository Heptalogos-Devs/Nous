"""Semantic source ordering and request grouping for every language entry point."""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass, replace
from typing import TYPE_CHECKING

from pydantic import JsonValue

from nous.common.errors import NousError
from nous.content.contracts import Artifact, SourceRole, SourceSelection
from nous.media.contracts import MediaPlan, PreparedBatch

from .tasks import TaskDefinition

if TYPE_CHECKING:
    from nous.media.configuration import MediaConfig
    from nous.media.preparation import MediaPreparer


@dataclass(frozen=True)
class RequestGroup:
    batches: tuple[PreparedBatch, ...]
    coverage: dict[str, JsonValue]


@dataclass(frozen=True)
class InputPlan:
    task_mode: str
    ordered_source_groups: tuple[Artifact, ...]
    representation_plans: tuple[MediaPlan, ...]
    request_groups: tuple[RequestGroup, ...]
    requested_coverage: dict[str, JsonValue]


async def plan_inputs(
    preparer: MediaPreparer,
    originals: Sequence[tuple[Artifact, SourceSelection, SourceRole]],
    settings: MediaConfig,
    *,
    requirements: Sequence[str] = (),
    selection: SourceSelection | None = None,
    document_mode: str = "auto",
    without_audio: bool = False,
    per_batch: bool = False,
    contract: TaskDefinition | None = None,
) -> InputPlan:
    mode = "independent_batches" if per_batch else contract.input_mode if contract else "joint"
    plans: list[MediaPlan] = []
    batches: list[PreparedBatch] = []
    chosen_inputs = tuple((artifact, selection or supplied) for artifact, supplied, _ in originals)
    task_plans = (
        await contract.prepare_sources(chosen_inputs, preparer.artifacts) if contract else None
    )
    for index, (artifact, supplied, role) in enumerate(originals, 1):
        chosen = selection or supplied
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
            )
        plans.append(plan)
        for batch in plan.batches:
            prefix = f"S{index}"
            preserve = contract is not None and contract.preserve_source_keys
            sources = tuple(
                replace(
                    source,
                    key=source.key if preserve else prefix + source.key[2:],
                    group=source.group.model_copy(
                        update={
                            "key": source.key if preserve else prefix + source.key[2:],
                            "role": role,
                        }
                    )
                    if source.group
                    else None,
                )
                for source in batch.sources
            )
            batch = replace(
                batch,
                sources=sources,
                inputs=tuple(source.group for source in sources if source.group),
                source_prefix=prefix,
                source_role=role,
            )
            batches.append(batch)
    coverage: dict[str, JsonValue] = {
        "mode": mode,
        "inputs": [
            {"artifact": plan.artifact.ref.model_dump(mode="json"), **plan.coverage}
            for plan in plans
        ],
    }
    requests = (
        tuple(RequestGroup((batch,), batch.coverage) for batch in batches)
        if mode == "independent_batches"
        else (RequestGroup(tuple(batches), coverage),)
    )
    return InputPlan(
        mode, tuple(artifact for artifact, _, _ in originals), tuple(plans), requests, coverage
    )


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
    return PreparedBatch(
        tuple(source for batch in batches for source in batch.sources),
        tuple(group for batch in batches for group in batch.inputs),
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
    """Plan only the selected route's representation over the same source ranges."""
    alternatives: list[PreparedBatch] = []
    for batch in request.batches:
        if batch.conversion is not None:
            recipe = batch.conversion
            plan = await preparer.prepare(
                recipe.artifact,
                recipe.selection,
                requirements=requirements,
                settings=recipe.settings,
                document_mode=recipe.document_mode,
                without_audio=recipe.without_audio,
            )
            alternatives.extend(
                replace(item, source_prefix=batch.source_prefix, source_role=batch.source_role)
                for item in plan.batches
            )
            continue
        for source in batch.sources:
            binary = (
                source.media
                or source.group
                and any(
                    part.media_type
                    and not part.media_type.startswith("text/")
                    and part.media_type != "application/json"
                    for part in source.group.parts
                )
            )
            if not binary or source.group is None:
                alternatives.append(
                    replace(
                        batch, sources=(source,), inputs=(source.group,) if source.group else ()
                    )
                )
                continue
            origins = tuple(
                dict.fromkeys(
                    origin.origin for part in source.group.parts for origin in part.origins
                )
            )
            for content in origins or tuple(part.content for part in source.group.parts):
                plan = await preparer.prepare(
                    content.artifact,
                    content.selection,
                    requirements=requirements,
                    settings=settings,
                )
                prefix = source.key.split(":", 1)[0]
                alternatives.extend(
                    replace(item, source_prefix=prefix, source_role=source.group.role)
                    for item in plan.batches
                )
    return RequestGroup(tuple(alternatives), request.coverage)
