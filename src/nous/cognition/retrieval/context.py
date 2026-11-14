"""Select explicit work, prior delivery and anchor context for query stages."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous.application.contracts import CallContext
from nous.cognition.contracts import NamespaceRef
from nous.cognition.material_contracts import (
    MaterialSelection,
)
from nous.cognition.work.contracts import ListWorkUses, WorkAttributes
from nous.cognition.work.refs import WorkRevisionRef
from nous.common.errors import NousError
from nous.content.contracts import (
    ResultOutputRef,
)
from nous.wave.contracts import (
    ContextContent,
    ContextFragment,
)

from .contracts import (
    ContextSelection,
    EvidenceDelivery,
)

if TYPE_CHECKING:
    from nous.cognition.materials import Materials
    from nous.cognition.scope import NamespaceOperations
    from nous.cognition.work.operations import WorkOperations
    from nous.content.artifacts import ArtifactStore
    from nous.content.results import ResultStore


class ContextPreparer:
    """Prepare the explicit context and its exact material references."""

    def __init__(
        self,
        work: WorkOperations,
        scope: NamespaceOperations,
        materials: Materials,
        results: ResultStore,
        artifacts: ArtifactStore,
    ):
        self.work, self.scope, self.materials = work, scope, materials
        self.results, self.artifacts = results, artifacts

    async def prepare(
        self,
        selection: ContextSelection,
        namespace: NamespaceRef,
        anchors: tuple[MaterialSelection, ...],
        call: CallContext,
    ) -> tuple[tuple[ContextFragment, ...], dict[str, MaterialSelection]]:
        fragments = list(selection.fragments)
        sources: dict[str, MaterialSelection] = {}
        default_stages: dict[str, tuple[str, ...]] = {}
        window = min(call.settings.values.wave.context.window, 200)
        if selection.work is not None:
            work = await self.materials.resolve_identifier(selection.work, namespace=namespace)
            if not isinstance(work, WorkRevisionRef):
                raise NousError("input.invalid_context", "Work context requires a work reference")
            view = await self.materials.resolve(work, namespace=namespace)
            attributes = WorkAttributes.model_validate(view.attributes.values)
            state, fields = attributes.state, attributes.fields
            choices: list[tuple[str, str]] = [("objective", state.objective)]
            if state.focus:
                choices.append(("focus", state.focus))
            candidates = [
                ("decision:" + item.key, item.statement)
                for item in state.decisions
                if item.state == "active"
            ]
            candidates += [
                ("question:" + item.key, item.question)
                for item in state.questions
                if item.state == "open"
            ]
            choices.extend(candidates[: max(0, window - len(choices))])
            parent = "material:" + self.materials.key(work)
            for key, value in choices:
                identity = f"{parent}:{key}"
                field = fields.get(key)
                fragments.append(
                    ContextFragment(
                        key=identity,
                        role="work",
                        stage="retrieval",
                        parent_key=parent,
                        sequence=0,
                        text=None if field else value,
                        content=ContextContent(
                            artifact=field.artifact.identifier, selector=field.selection
                        )
                        if field
                        else None,
                        validity=1.0,
                        half_life=None,
                    )
                )
                sources[identity] = MaterialSelection(
                    material=work,
                    selections=(field,) if field else (),
                )
                if key in {"objective", "focus"} or key.startswith("decision:"):
                    default_stages[identity] = ("retrieval", "localization")
            uses = await self.work.list_uses(
                ListWorkUses(ref=work.work_ref, namespace=namespace, limit=max(window, 1)),
                call,
            )
            explicit = [*state.active]
            explicit.extend(
                basis
                for decision in state.decisions
                if decision.state == "active"
                for basis in decision.basis
            )
            active: list[tuple[MaterialSelection, int, float | None, bool]] = [
                (item, 0, None, True) for item in explicit[:window]
            ]
            half_life = call.settings.values.cognition.search.active_use_half_life
            active.extend(
                (item, age, half_life, False)
                for age, use in enumerate(uses)
                for item in use.selections
            )
            seen: set[MaterialSelection] = set()
            unique: list[tuple[int, MaterialSelection, int, float | None, bool]] = []
            for index, (choice, age, lifetime, localize) in enumerate(active):
                if choice in seen:
                    continue
                seen.add(choice)
                unique.append((index, choice, age, lifetime, localize))
            selections = await self.materials.selections(
                tuple(choice for _, choice, _, _, _ in unique), namespace=namespace
            )
            text = iter(
                await self.artifacts.text_capabilities(
                    tuple(part for _, parts in selections for part in parts)
                )
            )
            for (index, choice, age, lifetime, localize), (_, parts) in zip(
                unique, selections, strict=True
            ):
                parent = "material:" + self.materials.key(choice.material)
                for part_index, part in enumerate(parts):
                    if not next(text) or (
                        part.selection.start_byte is not None
                        and part.selection.start_byte == part.selection.end_byte
                    ):
                        continue
                    identity = f"work-active:{index}:{part_index}"
                    fragments.append(
                        ContextFragment(
                            key=identity,
                            role="active_memory",
                            stage="retrieval",
                            parent_key=parent,
                            sequence=-age,
                            content=ContextContent(
                                artifact=part.artifact.identifier,
                                selector=part.selection,
                            ),
                            distance=age,
                            half_life=lifetime,
                        )
                    )
                    sources[identity] = MaterialSelection(
                        material=choice.material, selections=(part,)
                    )
                    if localize:
                        default_stages[identity] = ("retrieval", "localization")
        for result_ref in selection.prior_deliveries:
            artifact = await self.results.resolve_output(
                ResultOutputRef(result=result_ref, output="evidence")
            )
            delivery = EvidenceDelivery.model_validate_json(
                await self.artifacts.read_text(artifact)
            )
            await self.scope.resolve_for(namespace, delivery.intent.constraints.namespace)
            for index, passage in enumerate(delivery.passages[:window]):
                parent = "material:" + self.materials.key(passage.material)
                for part_index, part in enumerate((*passage.fragments, *passage.support_context)):
                    identity = f"delivery:{result_ref.id}:{passage.key}:{part_index}"
                    fragments.append(
                        ContextFragment(
                            key=identity,
                            role="prior_delivery",
                            stage="retrieval",
                            parent_key=parent,
                            sequence=-index,
                            content=ContextContent(
                                artifact=part.content.artifact.identifier,
                                selector=part.content.selection,
                            ),
                        )
                    )
                    sources[identity] = MaterialSelection(
                        material=passage.material, selections=(part.content,)
                    )
        if anchors:
            selections = await self.materials.selections(anchors, namespace=namespace)
            text = iter(
                await self.artifacts.text_capabilities(
                    tuple(part for _, parts in selections for part in parts)
                )
            )
            for index, (view, parts) in enumerate(selections):
                parent = "material:" + self.materials.key(view.ref)
                for part_index, part in enumerate(parts):
                    if not next(text):
                        raise NousError(
                            "input.unsupported_context",
                            "This context selection has no readable text",
                        )
                    identity = f"anchor:{index}:{part_index}"
                    fragments.append(
                        ContextFragment(
                            key=identity,
                            role="anchor",
                            stage="retrieval",
                            parent_key=parent,
                            sequence=part_index,
                            content=ContextContent(
                                artifact=part.artifact.identifier,
                                selector=part.selection,
                            ),
                        )
                    )
                    sources[identity] = MaterialSelection(
                        material=view.ref, selections=(part,)
                    )
        if len({item.key for item in fragments}) != len(fragments):
            raise NousError("input.invalid_context", "Context keys must be unique")
        prepared: list[ContextFragment] = []
        selected: dict[str, MaterialSelection] = {}
        for item in fragments:
            if item.key not in sources:
                prepared.append(item)
                continue
            stages = selection.stages or default_stages.get(item.key, ("retrieval",))
            for stage in dict.fromkeys(stages):
                key = item.key if stage == "retrieval" else f"{item.key}:{stage}"
                prepared.append(item.model_copy(update={"key": key, "stage": stage}))
                selected[key] = sources[item.key]
        fragments, sources = prepared, selected
        if len({item.key for item in fragments}) != len(fragments):
            raise NousError("input.invalid_context", "Context keys must be unique")
        supported = {"retrieval", "localization", "evaluation"}
        unknown = {item.stage for item in fragments} - supported
        if unknown:
            raise NousError(
                "input.unsupported_context_stage",
                "Context stage is not available",
                details={"stages": sorted(unknown)},
            )
        return tuple(fragments), sources
