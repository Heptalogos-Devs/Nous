"""Select explicit work, prior delivery and anchor context for query stages."""

from __future__ import annotations

from difflib import SequenceMatcher
from typing import TYPE_CHECKING

from nous.application.contracts import CallContext
from nous.cognition.contracts import NamespaceRef
from nous.cognition.material_contracts import (
    MaterialSelection,
)
from nous.cognition.work.contracts import ListWorkUses
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
    UsedContext,
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
        text: str,
        namespace: NamespaceRef,
        anchors: tuple[MaterialSelection, ...],
        call: CallContext,
    ) -> tuple[tuple[ContextFragment, ...], dict[str, UsedContext]]:
        fragments = list(selection.fragments)
        used: dict[str, UsedContext] = {}
        default_stages: dict[str, tuple[str, ...]] = {}
        window = min(call.settings.values.wave.context.window, 200)
        if selection.work is not None:
            view = await self.work.read_revision(selection.work)
            await self.scope.resolve_for(
                namespace, NamespaceRef(instance_id=view.ref.instance_id, id=view.ref.namespace_id)
            )
            choices: list[tuple[str, str]] = [("objective", view.state.objective)]
            if view.state.focus:
                choices.append(("focus", view.state.focus))
            candidates = [
                ("decision:" + item.key, item.statement)
                for item in view.state.decisions
                if item.state == "active"
            ]
            candidates += [
                ("question:" + item.key, item.question)
                for item in view.state.questions
                if item.state == "open"
            ]
            candidates.sort(
                key=lambda item: SequenceMatcher(None, text.casefold(), item[1].casefold()).ratio(),
                reverse=True,
            )
            choices.extend(candidates[: max(0, window - len(choices))])
            parent = "material:" + self.materials.key(view.revision_ref)
            for key, value in choices:
                identity = f"{parent}:{key}"
                field = view.fields.get(key)
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
                used[identity] = UsedContext(
                    key=identity,
                    source=MaterialSelection(
                        material=view.revision_ref,
                        selections=(view.fields[key],) if key in view.fields else (),
                    ),
                    text=value,
                    role="work",
                    stage="retrieval",
                    parent_key=parent,
                )
                if key in {"objective", "focus"} or key.startswith("decision:"):
                    default_stages[identity] = ("retrieval", "localization")
            uses = await self.work.list_uses(
                ListWorkUses(ref=view.ref, namespace=namespace, limit=max(window, 1)), call
            )
            explicit = [*view.state.active]
            explicit.extend(
                basis
                for decision in view.state.decisions
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
            seen: set[str] = set()
            unique: list[tuple[int, MaterialSelection, int, float | None, bool]] = []
            for index, (choice, age, lifetime, localize) in enumerate(active):
                signature = choice.model_dump_json()
                if signature in seen:
                    continue
                seen.add(signature)
                unique.append((index, choice, age, lifetime, localize))
            reads = await self.materials.read_many(
                tuple(choice for _, choice, _, _, _ in unique), namespace=namespace
            )
            for (index, choice, age, lifetime, localize), read in zip(unique, reads, strict=True):
                parent = "material:" + self.materials.key(choice.material)
                for part_index, part in enumerate(read.selections):
                    if not part.body_text:
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
                                artifact=part.source.artifact.identifier,
                                selector=part.source.selection,
                            ),
                            distance=age,
                            half_life=lifetime,
                        )
                    )
                    used[identity] = UsedContext(
                        key=identity,
                        source=MaterialSelection(
                            material=choice.material, selections=(part.source,)
                        ),
                        text=part.body_text,
                        role="active_memory",
                        stage="retrieval",
                        parent_key=parent,
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
                    used[identity] = UsedContext(
                        key=identity,
                        source=MaterialSelection(
                            material=passage.material, selections=(part.content,)
                        ),
                        text=part.text,
                        role="prior_delivery",
                        stage="retrieval",
                        parent_key=parent,
                    )
        if anchors:
            reads = await self.materials.read_many(anchors, namespace=namespace)
            for index, read in enumerate(reads):
                parent = "material:" + self.materials.key(read.material.ref)
                for part_index, part in enumerate(read.selections):
                    if part.body_text is None:
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
                                artifact=part.source.artifact.identifier,
                                selector=part.source.selection,
                            ),
                        )
                    )
                    used[identity] = UsedContext(
                        key=identity,
                        source=MaterialSelection(
                            material=read.material.ref, selections=(part.source,)
                        ),
                        text=part.body_text,
                        role="anchor",
                        stage="retrieval",
                        parent_key=parent,
                    )
        if len({item.key for item in fragments}) != len(fragments):
            raise NousError("input.invalid_context", "Context keys must be unique")
        prepared: list[ContextFragment] = []
        selected: dict[str, UsedContext] = {}
        for item in fragments:
            if item.key not in used:
                prepared.append(item)
                continue
            stages = selection.stages or default_stages.get(item.key, ("retrieval",))
            for stage in dict.fromkeys(stages):
                key = item.key if stage == "retrieval" else f"{item.key}:{stage}"
                prepared.append(item.model_copy(update={"key": key, "stage": stage}))
                selected[key] = used[item.key].model_copy(update={"key": key, "stage": stage})
        fragments, used = prepared, selected
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
        return tuple(fragments), used
