"""Arrange a saved delivery and, when requested, add bounded adjacent text."""

import asyncio
import json
from typing import TYPE_CHECKING

from nous.application.contracts import CallContext, LanguageEvents, Progress, define_operation
from nous.cognition.material_contracts import MaterialSource
from nous.cognition.materials import Materials
from nous.cognition.retrieval.contracts import DeliveryUsage, EvidenceDelivery, QueryControls
from nous.common.errors import NousError
from nous.common.operations import (
    OperationIssue,
    add_error_context,
    record_completed,
    report_outcome,
)
from nous.content.contracts import (
    ContentSelection,
    ResultOutputRef,
    SelectedInputs,
    SelectedSource,
    SourceSelection,
)
from nous.content.results import ResultStore
from nous.content.selections import subtract_selection
from nous.models.generation import GenerationOperations, GenerationRequest
from nous.wave.contracts import DeliveredText

from .contracts import ArrangeReading, ReadingBrief, ReadingContextRequest, ReadingOutcome

if TYPE_CHECKING:
    from nous.cognition.retrieval.adoption import EvidenceAdoption
    from nous.wave.operations import WaveOperations


ARRANGE = define_operation(
    "steward.arrange",
    request=ArrangeReading,
    result=ReadingOutcome,
    events=LanguageEvents,
    description="Arrange a saved evidence delivery for a reading purpose.",
)


class StewardOperations:
    """Arrange saved material and adopt any explicitly requested adjacent context."""

    def __init__(
        self,
        generation: GenerationOperations,
        results: ResultStore,
        materials: Materials,
        evidence_adoption: EvidenceAdoption,
        wave: WaveOperations,
    ) -> None:
        self.generation, self.results, self.materials = generation, results, materials
        self.evidence_adoption, self.wave = evidence_adoption, wave

    async def _request(
        self, request: ArrangeReading, delivery: EvidenceDelivery, *, expansion_allowed: bool
    ) -> GenerationRequest:
        materials = {item.material.ref: item.material for item in delivery.materials}
        sources: list[SelectedSource] = []
        catalogue: list[dict[str, object]] = []
        for passage in delivery.passages:
            material = materials.get(passage.material)
            if not passage.selections:
                raise NousError(
                    "steward.invalid_evidence", "An evidence item has no readable selections"
                )
            normalized = await self.results.artifacts.normalize_selections(passage.selections)
            title = material.title if material else None
            sources.append(
                SelectedSource(
                    slot_key=passage.key,
                    selections=passage.selections,
                    display_name=f"{passage.key}: {title or 'Evidence'}",
                )
            )
            catalogue.append(
                {
                    "key": passage.key,
                    "title": title,
                    "times": material.times.model_dump(mode="json")
                    if material and material.times
                    else None,
                    "heading_paths": list(
                        dict.fromkeys(
                            section.heading_path
                            for section in material.sections
                            if any(
                                subtract_selection(section.selection, (item,))
                                != (section.selection,)
                                for item in normalized
                            )
                        )
                    )
                    if material
                    else [],
                }
            )

        def controls(value: QueryControls) -> dict[str, object]:
            return value.model_dump(
                mode="json",
                exclude_none=True,
                exclude_defaults=True,
                include={
                    "selectors",
                    "limit",
                    "return_types",
                    "times",
                    "time_clauses",
                    "prefer",
                    "avoid",
                    "sort",
                    "explore",
                    "include_review",
                    "include_exploration",
                },
            )

        intent = delivery.intent
        context = [
            {"role": role, "text": text}
            for role, text in dict.fromkeys(
                (item.role, item.text) for item in intent.context if item.text
            )
        ]
        instructions = json.dumps(
            {
                "purpose": request.purpose,
                "detail": request.detail,
                "output_language": request.output_language,
                "aspects": list(intent.aspects),
                "constraints": {
                    "controls": controls(intent.constraints.controls),
                    "branches": [
                        controls(branch.controls) for branch in intent.constraints.branches
                    ],
                },
                "context": context,
                "evidence": catalogue,
                "expansion_allowed": expansion_allowed,
            },
            ensure_ascii=False,
            indent=2,
        )
        return GenerationRequest(
            task=request.task or "arrange-reading",
            inputs=SelectedInputs(sources=tuple(sources)),
            question=intent.original_text,
            instructions=instructions,
        )

    async def _expand(
        self, delivery: EvidenceDelivery, requests: tuple[ReadingContextRequest, ...]
    ) -> tuple[EvidenceDelivery | None, tuple[OperationIssue, ...]]:
        passages = {item.key: item for item in delivery.passages}
        materials = {item.material.ref: item.material for item in delivery.materials}
        budget = delivery.program_run.budget if delivery.program_run else delivery.budget
        existing = tuple(
            selection for passage in delivery.passages for selection in passage.selections
        )
        counter = await self.wave.delivery.selection_budget(
            budget, existing, program_run=delivery.program_run
        )
        if await self.generation.execution.blocking(counter.add, existing) is None:
            return None, (
                OperationIssue(
                    stage="steward.expand",
                    code="steward.budget_exhausted",
                    message="The existing delivery already exhausts its budget",
                ),
            )

        unavailable: set[str] = set()
        exceeded: set[str] = set()
        changed = False
        for request in dict.fromkeys(requests):
            passage = passages[request.key]
            material = materials.get(passage.material)
            body = material.body if material else None
            if body is None or body.artifact.id not in counter.views:
                unavailable.add(request.key)
                continue
            view = counter.views[body.artifact.id]
            anchors = tuple(
                item
                for item in (
                    passage.scored_ranges or tuple(part.content for part in passage.fragments)
                )
                if item.artifact == body.artifact
            )
            if not anchors:
                unavailable.add(request.key)
                continue
            candidate = await self.generation.execution.blocking(
                view.adjacent, anchors, request.direction, within=body
            )
            if candidate is None:
                unavailable.add(request.key)
                continue
            own = tuple(
                ContentSelection(
                    artifact=item.artifact,
                    selection=SourceSelection(start_byte=bounds[0], end_byte=bounds[1]),
                )
                for item in passage.selections
                if item.artifact == body.artifact
                for bounds in (view.selection_bounds(item),)
            )
            needed = subtract_selection(candidate, own)
            if not needed:
                unavailable.add(request.key)
                continue
            added = await self.generation.execution.blocking(counter.add, needed)
            if added is None:
                exceeded.add(request.key)
                continue
            newly_read = await self.results.artifacts.read_selections(added)
            shared = tuple(
                part for selection in needed for part in subtract_selection(selection, added)
            )
            passages[request.key] = passage.model_copy(
                update={
                    "support_context": (
                        *passage.support_context,
                        *(
                            DeliveredText(content=item.content, text=item.text)
                            for item in newly_read
                        ),
                    ),
                    "shared_context": (*passage.shared_context, *shared),
                }
            )
            changed = True
        issues = tuple(
            OperationIssue(
                stage="steward.expand",
                code=code,
                message=message,
                affected_refs=tuple(sorted(keys)),
            )
            for keys, code, message in (
                (unavailable, "steward.context_unavailable", "No adjacent text is available"),
                (
                    exceeded,
                    "steward.budget_exhausted",
                    "Adjacent text does not fit the delivery budget",
                ),
            )
            if keys
        )
        if not changed:
            return None, issues
        return delivery.model_copy(
            update={
                "formed_at": self.results.artifacts.clock.utc_now(),
                "passages": tuple(passages[item.key] for item in delivery.passages),
                "delivery_usage": DeliveryUsage(
                    unit=budget.unit if budget else "characters", amount=counter.used
                ),
            }
        ), issues

    async def arrange(self, request: ArrangeReading, call: CallContext) -> ReadingOutcome:
        async with self.generation.execution.operation("steward.arrange"):
            record_completed(request.evidence)
            evidence_refs = [request.evidence]
            try:
                artifact = await self.results.resolve_output(
                    ResultOutputRef(result=request.evidence, output="evidence")
                )
                delivery = EvidenceDelivery.model_validate_json(
                    await self.results.artifacts.read_text(artifact)
                )
                keys = [passage.key for passage in delivery.passages]
                if not keys or len(keys) != len(set(keys)):
                    raise NousError(
                        "steward.invalid_evidence", "Evidence keys must be nonempty and unique"
                    )
                task = request.task or "arrange-reading"
                self.generation.require_contract(task, "arrange-reading", call.settings)
                issues: list[OperationIssue] = []
                completed = None
                brief = None
                for may_expand in (True, False):
                    await call.events.emit(Progress(stage="arrange"))
                    completed = await self.generation.compute(
                        await self._request(request, delivery, expansion_allowed=may_expand), call
                    )
                    output = completed.outputs.get("reading")
                    if output is None:
                        raise NousError("steward.invalid_output", "Reading task returned no brief")
                    brief = ReadingBrief.model_validate_json(output.data)
                    if not brief.context_requests:
                        break
                    if not may_expand:
                        issues.append(
                            OperationIssue(
                                stage="steward.expand",
                                code="steward.expansion_limit",
                                message="Only one adjacent-context expansion is available",
                            )
                        )
                        break
                    expanded, expansion_issues = await self._expand(
                        delivery, brief.context_requests
                    )
                    issues.extend(expansion_issues)
                    if expanded is None:
                        break
                    evidence_refs.append(
                        await self.evidence_adoption.save(
                            expanded, call, dependencies=(evidence_refs[-1],)
                        )
                    )
                    delivery = expanded
                assert completed is not None and brief is not None
                selections = {item.key: item.sources for item in completed.facts.selections}
                materials = {item.material.ref: item.material for item in delivery.materials}
                formal_sources = tuple(
                    MaterialSource(
                        key=passage.key,
                        material=passage.material,
                        selections=selections[passage.key],
                        name=materials[passage.material].title
                        if passage.material in materials
                        else None,
                    )
                    for passage in delivery.passages
                    if selections.get(passage.key)
                )
                facts = completed.facts.model_copy(
                    update={
                        "dependencies": tuple(
                            dict.fromkeys((*completed.facts.dependencies, evidence_refs[-1]))
                        )
                    }
                )
                await call.events.emit(Progress(stage="save_reading"))
                prepared = await self.results.prepare_outputs(completed.outputs)
                async with self.results.database.write() as tx:
                    saved = await self.results.adopt_in(tx, facts=facts, prepared_outputs=prepared)
                    await self.materials.adopt_sources_in(
                        tx,
                        saved,
                        ("reading", "markdown"),
                        formal_sources,
                        namespace=delivery.intent.constraints.namespace,
                    )
                await call.saved(saved.ref)
                if issues:
                    report_outcome("partial", issues=issues)
                return ReadingOutcome(
                    evidence_ref=evidence_refs[-1],
                    reading_ref=saved.ref,
                    brief=brief,
                    issues=tuple(issues),
                )
            except (NousError, ValueError, asyncio.CancelledError, TimeoutError) as error:
                add_error_context(
                    error,
                    {
                        "evidence_ref": evidence_refs[-1].model_dump(mode="json"),
                        "completed_refs": [ref.model_dump(mode="json") for ref in evidence_refs],
                    },
                )
                raise
