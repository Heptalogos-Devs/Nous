"""Semantic source ordering and request grouping for every language entry point."""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass, replace
from typing import TYPE_CHECKING

from pydantic import JsonValue

from nous.common.errors import NousError
from nous.content.contracts import Artifact, InputGroup, InputPart, SourceSelection
from nous.media.contracts import MediaPlan, PreparedBatch, PreparedSource

from .tasks import TASKS

if TYPE_CHECKING:
    from nous.media.configuration import MediaConfig
    from nous.media.preparation import MediaPreparer

    from .executor import FrozenTask


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
    originals: Sequence[tuple[Artifact, SourceSelection]],
    frozen: FrozenTask | None,
    settings: MediaConfig,
    *,
    requirements: Sequence[str] = (),
    selection: SourceSelection | None = None,
    document_mode: str = "auto",
    without_audio: bool = False,
    per_batch: bool = False,
) -> InputPlan:
    contract = TASKS[frozen.resource.manifest.contract_id] if frozen and frozen.resource else None
    mode = "independent_batches" if per_batch else contract.input_mode if contract else "joint"
    plans: list[MediaPlan] = []
    batches: list[PreparedBatch] = []
    for index, (artifact, supplied) in enumerate(originals, 1):
        chosen = selection or supplied
        if contract and contract.id == "concept-annotation":
            if len(originals) != 1:
                raise NousError("model.annotation_body", "Concept annotation requires one body")
            if chosen != SourceSelection():
                raise NousError(
                    "model.annotation_body",
                    "Concept annotation requires one complete immutable body",
                )
            view = await preparer.artifacts.text_view(artifact.ref)
            roots = [block for block in view.blocks if block.parent_key is None]
            sources = tuple(
                PreparedSource(
                    block.key,
                    artifact.display_name,
                    text=block.text,
                    group=InputGroup(
                        key=block.key,
                        name=artifact.display_name,
                        parts=(
                            InputPart(
                                content=block.selection,
                                representation="text",
                                media_type=artifact.media_type,
                                source_media_type=artifact.media_type,
                            ),
                        ),
                    ),
                )
                for block in roots
            )
            batch = PreparedBatch(
                sources,
                tuple(source.group for source in sources if source.group),
                chosen,
                {
                    "body_ref": artifact.ref.model_dump(mode="json"),
                    "parser_profile": view.parser_profile,
                },
            )
            plan = MediaPlan(artifact, chosen, (batch,), batch.coverage)
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
            if not (contract and contract.id == "concept-annotation"):
                sources = tuple(
                    replace(
                        source,
                        key=prefix + source.key[2:],
                        group=source.group.model_copy(update={"key": prefix + source.key[2:]})
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
        mode, tuple(artifact for artifact, _ in originals), tuple(plans), requests, coverage
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
