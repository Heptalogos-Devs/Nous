"""Inspect prompt plans and probe configured models without adopting extra behavior."""

from __future__ import annotations

from collections.abc import Sequence
from pathlib import Path
from typing import TYPE_CHECKING, cast

from pydantic import JsonValue

from nous.common.errors import NousError
from nous.content.contracts import SelectedInputs
from nous.models.configuration import ModelRoute, PresetConfig, TaskAssignment

from ..contracts import ReadingInput
from ..input import materialize, plan_inputs
from ..requests import FrozenTask
from .contracts import GenerationOutcome

if TYPE_CHECKING:
    from nous.configuration import ConfigSnapshot

    from .operations import GenerationOperations, InputValue


async def preview(
    owner: GenerationOperations,
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
    async with owner.execution.operation("prompt.preview", snapshot):
        resource = owner.executor.compiler.catalog.get(prompt)
        frozen = owner.executor.freeze(task, snapshot) if task else None
        if frozen is not None and (frozen.resource is None or frozen.resource.id != resource.id):
            raise NousError(
                "model.prompt_conflict",
                "Prompt preview must use the selected task's prompt resource",
            )
        selected = (
            await owner.accept_inputs(input, snapshot, texts=texts)
            if input or texts
            else SelectedInputs(sources=())
        )
        if frozen and selected.sources:
            prepared = await owner.prepare_input(
                frozen,
                selected,
                snapshot,
                per_batch=per_batch,
            )
            plan, frozen = prepared.input_plan, prepared.frozen
        else:
            plan = await plan_inputs(
                owner.preparer,
                selected,
                snapshot.values.media,
                requirements=frozen.routes[0].model.input_formats if frozen else (),
                per_batch=per_batch,
                contract=owner.executor.compiler.catalog.tasks[resource.manifest.contract_id],
            )
        previews: list[JsonValue] = []
        for group in plan.request_groups:
            if frozen:
                request = await owner.preparation.prepare_group(
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
                    owner.preparer, group, snapshot.values.media.max_inline_bytes
                )
                previews.append(
                    owner.executor.compiler.compile(
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
    owner: GenerationOperations, model: str, input: Path | None, snapshot: ConfigSnapshot
) -> GenerationOutcome:
    settings = snapshot.values
    selected = settings.models.get(model)
    if selected is None:
        raise NousError("model.unknown", "The model is not configured", details={"model": model})
    assignment = TaskAssignment(model=model, prompt="describe-media")
    frozen = FrozenTask(
        "model.probe",
        assignment,
        (ModelRoute(model, selected, PresetConfig()),),
        owner.executor.compiler.catalog.get("describe-media"),
    )
    return await owner.run_input(
        frozen,
        [input] if input else [],
        snapshot,
        texts=() if input else ("Describe the supplied content briefly.",),
    )
