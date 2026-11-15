"""Select explicit work, prior delivery and anchor context for query stages."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous.application.contracts import CallContext
from nous.cognition.contracts import NamespaceRef
from nous.cognition.material_contracts import (
    MaterialSelection,
)
from nous.common.errors import NousError
from nous.wave.contracts import (
    ContextContent,
    ContextFragment,
)

from .adoption import read_saved_evidence
from .contracts import ContextSelection

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
            work_context = await self.work.context(selection.work, namespace, window)
            work = work_context.ref
            parent = "material:" + self.materials.key(work)
            for field in work_context.fields:
                identity = f"{parent}:{field.key}"
                fragments.append(
                    ContextFragment(
                        key=identity,
                        role="work",
                        stage="retrieval",
                        parent_key=parent,
                        sequence=0,
                        content=ContextContent(
                            artifact=field.selection.artifact.identifier,
                            selector=field.selection.selection,
                        ),
                        validity=1.0,
                        half_life=None,
                    )
                )
                sources[identity] = MaterialSelection(material=work, selections=(field.selection,))
                if field.key in {"objective", "focus"} or field.key.startswith("decision:"):
                    default_stages[identity] = ("retrieval", "localization")
            active: list[tuple[MaterialSelection, int, float | None, bool]] = [
                (item, 0, None, True) for item in work_context.active
            ]
            half_life = call.settings.values.cognition.search.active_use_half_life
            active.extend(
                (item, age, half_life, False)
                for age, group in enumerate(work_context.recent)
                for item in group
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
            result = await read_saved_evidence(self.results, result_ref)
            await self.scope.resolve_for(namespace, result.namespace)
            prior_sources = result.sources[:window]
            capabilities = iter(
                await self.artifacts.text_capabilities(
                    tuple(part for source in prior_sources for part in source.selections)
                )
            )
            for index, source in enumerate(prior_sources):
                parent = "material:" + self.materials.key(source.material)
                for part_index, part in enumerate(source.selections):
                    if not next(capabilities):
                        continue
                    identity = f"delivery:{result_ref.id}:{source.key}:{part_index}"
                    fragments.append(
                        ContextFragment(
                            key=identity,
                            role="prior_delivery",
                            stage="retrieval",
                            parent_key=parent,
                            sequence=-index,
                            content=ContextContent(
                                artifact=part.artifact.identifier,
                                selector=part.selection,
                            ),
                        )
                    )
                    sources[identity] = MaterialSelection(
                        material=source.material, selections=(part,)
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
                    sources[identity] = MaterialSelection(material=view.ref, selections=(part,))
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
