"""Adopt a completed evidence delivery with its exact material sources."""

from __future__ import annotations

import json
from contextlib import AsyncExitStack
from dataclasses import dataclass
from typing import TYPE_CHECKING, cast

from pydantic import JsonValue, TypeAdapter

from nous.application.contracts import CallContext, Progress
from nous.cognition.contracts import NamespaceRef
from nous.common.errors import NousError
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import (
    ContentSelection,
    MaterialSource,
    OutputContent,
    ProgramProducer,
    ResultFacts,
    ResultOutputRef,
    ResultRef,
    SelectedInputs,
    SelectedSource,
)
from nous.content.results import ResultStore
from nous.content.selections import subtract_selection
from nous.storage.database import Database
from nous.wave.contracts import SnapshotRef
from nous.wave.operations import WaveOperations

from .contracts import EvidenceDelivery, QueryIntent, UsedContext

if TYPE_CHECKING:
    from nous.cognition.materials import Materials


@dataclass(frozen=True)
class SavedEvidence:
    ref: ResultRef
    namespace: NamespaceRef
    question: str
    aspects: tuple[str, ...]
    sources: tuple[MaterialSource, ...]
    snapshot: SnapshotRef | None
    context: tuple[dict[str, str], ...] | None = None

    def source(self, key: str) -> MaterialSource:
        match = next((source for source in self.sources if source.key == key), None)
        if match is None:
            raise NousError("cognition.selection_mismatch", "Evidence key is not in this delivery")
        return match


async def read_saved_evidence(
    results: ResultStore, ref: str | ResultRef, *, include_context: bool = False
) -> SavedEvidence:
    """Read the adopted evidence facts, restoring used context only when requested."""
    result_ref, operation, request, sources, evidence_sources = await results.source_facts(
        ref, "evidence"
    )
    if operation != "retrieval.query" or evidence_sources is None:
        raise NousError("cognition.invalid_evidence", "Result is not a query evidence delivery")
    intent = QueryIntent.model_validate(request.get("intent"))
    context: tuple[dict[str, str], ...] | None = None
    if include_context:
        by_key = {source.key: source for source in sources if source.kind == "context"}
        missing = [
            (item, by_key["context:" + item.key])
            for item in intent.context
            if item.text is None and "context:" + item.key in by_key
        ]
        selections = tuple(selection for _, source in missing for selection in source.selections)
        values = iter(await results.artifacts.read_selections(selections))
        text_by_key = {
            item.key: "".join(next(values).text for _ in source.selections)
            for item, source in missing
        }
        context = tuple(
            {
                "role": item.role,
                "text": item.text if item.text is not None else text_by_key[item.key],
            }
            for item in intent.context
            if item.text or item.key in text_by_key
        )
    return SavedEvidence(
        ref=result_ref,
        namespace=intent.namespace,
        question=intent.original_text,
        aspects=intent.aspects,
        sources=evidence_sources,
        snapshot=TypeAdapter(SnapshotRef | None).validate_python(request.get("snapshot")),
        context=context,
    )


class EvidenceAdoption:
    """Persist the same delivered evidence for query, feedback, and later work."""

    def __init__(
        self,
        database: Database,
        artifacts: ArtifactStore,
        results: ResultStore,
        materials: Materials,
        wave: WaveOperations,
    ) -> None:
        self.database, self.artifacts, self.results = database, artifacts, results
        self.materials, self.wave = materials, wave

    async def validate(self, evidence: EvidenceDelivery) -> EvidenceDelivery:
        """Validate a delivery supplied by an external caller before adoption."""
        namespace = evidence.intent.namespace
        if evidence.snapshot is not None and evidence.snapshot.instance_id != namespace.instance_id:
            raise NousError("cognition.namespace_mismatch", "Evidence belongs to another instance")
        keys = [part.key for item in evidence.materials for part in item.passages]
        if len(keys) != len(set(keys)):
            raise NousError("cognition.invalid_evidence", "Evidence keys must be unique")
        all_parts = tuple(part for item in evidence.materials for part in item.passages)
        for item in evidence.materials:
            for part in item.passages:
                own = tuple(view.source for view in part.views)
                other = tuple(
                    view.source
                    for candidate in all_parts
                    if candidate is not part
                    for view in candidate.views
                )
                if not (*own, *part.shared) or any(
                    subtract_selection(selection, other) for selection in part.shared
                ):
                    raise NousError(
                        "cognition.invalid_evidence", "Evidence does not match its material"
                    )
        async with self.database.read() as tx:
            await self.materials.capture_in(
                tx,
                (
                    *tuple(item.material for item in evidence.materials),
                    *tuple(link.selection.material for link in evidence.continuation),
                    *tuple(ref for link in evidence.continuation for ref in link.via),
                ),
                namespace=namespace,
            )
        delivered = tuple(
            view for item in evidence.materials for part in item.passages for view in part.views
        )
        actual = await self.artifacts.read_view_many(
            tuple(item.source for item in delivered), representations="none"
        )
        if any(
            left.body_text != right.body_text for left, right in zip(delivered, actual, strict=True)
        ):
            raise NousError("cognition.invalid_evidence", "Delivered text differs from its source")
        text_inputs = tuple(
            selection
            for item in evidence.intent.context
            if item.source is not None and item.source.selections and item.text is not None
            for selection in item.source.selections
        )
        actual_context = iter(await self.artifacts.read_selections(text_inputs))
        context: list[UsedContext] = []
        for item in evidence.intent.context:
            if item.source is None or not item.source.selections or item.text is None:
                context.append(item)
                continue
            original = "".join(next(actual_context).text for _ in item.source.selections)
            context.append(
                item.model_copy(update={"text": None}) if item.text == original else item
            )
        return evidence.model_copy(
            update={"intent": evidence.intent.model_copy(update={"context": tuple(context)})}
        )

    async def save(
        self,
        evidence: EvidenceDelivery,
        call: CallContext,
        *,
        sources: tuple[MaterialSource, ...],
        retain_snapshot: bool = False,
        dependencies: tuple[ResultRef, ...] = (),
    ) -> ResultRef:
        namespace = evidence.intent.namespace
        await call.events.emit(Progress(stage="evidence.save", completed=0, total=1))
        context_sources = tuple(
            MaterialSource(
                key="context:" + item.key,
                material=item.source.material,
                selections=item.source.selections,
                kind="context",
            )
            for item in evidence.intent.context
            if item.source is not None and item.source.selections
        )
        context: list[UsedContext] = []
        for item in evidence.intent.context:
            if item.source is None or not item.source.selections:
                context.append(item)
                continue
            context.append(item.model_copy(update={"source": None}))
        intent = evidence.intent.model_copy(update={"context": tuple(context)})
        source_dependencies = tuple(
            source.material.result
            for source in context_sources
            if isinstance(source.material, ResultOutputRef)
        )
        referenced = (
            *(item.material for item in evidence.materials),
            *(link.selection.material for link in evidence.continuation),
            *(ref for link in evidence.continuation for ref in link.via),
        )
        held = tuple(
            source.material.result
            for source in sources
            if isinstance(source.material, ResultOutputRef)
        )
        referenced_results = tuple(
            ref.result
            for ref in referenced
            if isinstance(ref, ResultOutputRef) and ref.result not in held
        )
        payload = evidence.model_dump(
            mode="json",
            exclude={"materials", "intent", "snapshot", "coverage", "program", "formed_at"},
        )
        evaluation_inputs: list[SelectedSource] = []
        if evidence.evaluation is not None:
            evaluation = dict(evidence.evaluation)
            inputs = evaluation.get("inputs", [])
            if not isinstance(inputs, list):
                raise NousError("cognition.invalid_evidence", "Invalid evaluation inputs")
            saved_inputs: list[JsonValue] = []
            for item in inputs:
                if not isinstance(item, dict) or not isinstance(item.get("slot_key"), str):
                    raise NousError("cognition.invalid_evidence", "Invalid evaluation input")
                slot_key = item["slot_key"]
                assert isinstance(slot_key, str)
                selections = TypeAdapter(tuple[ContentSelection, ...]).validate_python(
                    item.get("selections")
                )
                # Preserve order and each artifact identity when an input spans artifacts.
                groups: list[list[ContentSelection]] = []
                for selection in selections:
                    if not groups or groups[-1][0].artifact != selection.artifact:
                        groups.append([])
                    groups[-1].append(selection)
                keys: list[JsonValue] = []
                for index, group in enumerate(groups):
                    key = slot_key if len(groups) == 1 else f"{slot_key}:{index}"
                    keys.append(key)
                    evaluation_inputs.append(
                        SelectedSource(key=key, material=group[0].artifact, selections=tuple(group))
                    )
                saved_inputs.append(
                    {**{k: v for k, v in item.items() if k != "selections"}, "source_keys": keys}
                )
            evaluation["inputs"] = saved_inputs
            payload["evaluation"] = evaluation
        payload["matches"] = [
            {
                "material": item.material.model_dump(mode="json"),
                "rank": item.rank,
                "score": item.score,
                "score_kind": item.score_kind,
                "delivery_status": item.delivery_status,
                "needs_review": item.needs_review,
                "passages": [
                    {
                        "key": part.key,
                        **part.model_dump(
                            mode="json", exclude={"key", "views", "shared"}, exclude_none=True
                        ),
                    }
                    for part in item.passages
                ],
            }
            for item in evidence.materials
            if item.rank is not None
        ]
        data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        facts = ResultFacts(
            operation="retrieval.query",
            producer=ProgramProducer(),
            parameters={"program": evidence.program},
            material_sources=(*sources, *context_sources),
            selected_inputs=SelectedInputs(sources=tuple(evaluation_inputs)),
            request={
                "intent": intent.model_dump(mode="json", exclude_none=True),
                "snapshot": evidence.snapshot.model_dump(mode="json")
                if evidence.snapshot
                else None,
            },
            coverage=cast(dict[str, JsonValue], evidence.coverage) if evidence.coverage else {},
            dependencies=tuple(
                dict.fromkeys((*dependencies, *source_dependencies, *referenced_results))
            ),
            formed_at=evidence.formed_at or self.artifacts.clock.utc_now(),
        )
        prepared = await self.results.prepare_outputs(
            {
                "evidence": OutputContent(
                    data=data, media_type="application/json", display_name="evidence.json"
                )
            }
        )
        async with AsyncExitStack() as stack:
            if retain_snapshot and evidence.snapshot is not None:
                await stack.enter_async_context(
                    self.wave.snapshots.hold_reference(evidence.snapshot)
                )
            async with self.database.write() as tx:
                await self.materials.validate_sources_in(
                    tx, (*sources, *context_sources), namespace=namespace
                )
                saved = await self.results.adopt_in(
                    tx, facts=facts, prepared_outputs=prepared, output_sources={"evidence": sources}
                )
                if retain_snapshot and evidence.snapshot is not None:
                    await self.wave.retain_in(
                        tx, evidence.snapshot, owner_kind="result", owner_id=saved.ref.id
                    )
        await call.saved(saved.ref)
        await call.events.emit(Progress(stage="evidence.save", completed=1, total=1))
        return saved.ref
