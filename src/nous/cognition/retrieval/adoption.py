"""Adopt a completed evidence delivery with its exact material sources."""

from contextlib import AsyncExitStack

from nous.application.contracts import CallContext, Progress
from nous.cognition.material_contracts import MaterialSelection, MaterialSource
from nous.cognition.materials import Materials
from nous.cognition.scope import NamespaceOperations
from nous.common.errors import NousError
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import (
    InputGroup,
    InputPart,
    NamedSelection,
    OutputContent,
    OutputMapping,
    ProgramProducer,
    ResultFacts,
    ResultRef,
    SelectedInputs,
    SelectedSource,
    SourceSelection,
)
from nous.content.results import ResultStore
from nous.content.selections import subtract_selection
from nous.storage.database import Database
from nous.wave.operations import WaveOperations

from .contracts import EvidenceDelivery


class EvidenceAdoption:
    """Persist the same delivered evidence for query, feedback, and later work."""

    def __init__(
        self,
        database: Database,
        artifacts: ArtifactStore,
        results: ResultStore,
        materials: Materials,
        scope: NamespaceOperations,
        wave: WaveOperations,
    ) -> None:
        self.database, self.artifacts, self.results = database, artifacts, results
        self.materials, self.scope, self.wave = materials, scope, wave

    async def save(
        self, evidence: EvidenceDelivery, call: CallContext, *, retain_snapshot: bool = False
    ) -> ResultRef:
        evidence = evidence.model_copy(deep=True)
        namespace = evidence.intent.constraints.namespace
        await self.scope.resolve_for(None, namespace)
        if evidence.snapshot.instance_id != namespace.instance_id:
            raise NousError("cognition.namespace_mismatch", "Evidence belongs to another instance")
        keys = [passage.key for passage in evidence.passages]
        if len(keys) != len(set(keys)):
            raise NousError("cognition.invalid_evidence", "Evidence keys must be unique")
        assigned: dict[str, object] = {}
        for item in evidence.materials:
            for key in item.passage_keys:
                if key in assigned:
                    raise NousError(
                        "cognition.invalid_evidence", "Evidence key occurs more than once"
                    )
                assigned[key] = item.material.ref
        if set(assigned) != set(keys) or any(
            assigned[part.key] != part.material for part in evidence.passages
        ):
            raise NousError(
                "cognition.invalid_evidence", "Evidence keys do not match their materials"
            )
        if any(
            (part.candidate is not None and part.candidate.snapshot != evidence.snapshot.identifier)
            or not (part.fragments or part.support_context)
            or not part.selections
            for part in evidence.passages
        ):
            raise NousError("cognition.invalid_evidence", "Evidence contains an invalid passage")
        await self.materials.resolve_many(
            tuple(item.material.ref for item in evidence.materials), namespace=namespace
        )
        await self.materials.read_many(
            tuple(
                MaterialSelection(material=part.material, selections=part.selections)
                for part in evidence.passages
            ),
            namespace=namespace,
        )
        delivered = tuple(
            fragment
            for passage in evidence.passages
            for fragment in (*passage.fragments, *passage.support_context)
        )
        delivered_ranges = tuple(part.content for part in delivered)
        if any(
            subtract_selection(selection, delivered_ranges)
            for part in evidence.passages
            for selection in (*part.scored_ranges, *part.shared_context)
        ):
            raise NousError(
                "cognition.invalid_evidence", "Passage ranges exceed the delivered content"
            )
        actual = await self.artifacts.read_selections(tuple(item.content for item in delivered))
        if any(left.text != right.text for left, right in zip(delivered, actual, strict=True)):
            raise NousError("cognition.invalid_evidence", "Delivered text differs from its source")

        await call.events.emit(Progress(stage="evidence.save", completed=0, total=1))
        sources = tuple(
            SelectedSource(slot_key=part.key, selections=part.selections)
            for part in evidence.passages
        )
        data = evidence.model_dump_json().encode("utf-8")
        origins = tuple(dict.fromkeys(part for source in sources for part in source.selections))
        facts = ResultFacts(
            operation="retrieval.query",
            producer=ProgramProducer(
                operation="retrieval.query", parameters={"program": evidence.program}
            ),
            selected_inputs=SelectedInputs(sources=sources),
            inputs=tuple(
                InputGroup(
                    key=source.slot_key,
                    slot_key=source.slot_key,
                    name=source.slot_key,
                    parts=tuple(InputPart(content=part) for part in source.selections),
                )
                for source in sources
            ),
            mappings=(
                OutputMapping(
                    output_name="evidence",
                    selection=SourceSelection(start_byte=0, end_byte=len(data)),
                    relation="derived",
                    origins=origins,
                ),
            )
            if origins
            else (),
            selections=tuple(
                NamedSelection(key=source.slot_key, sources=source.selections) for source in sources
            ),
            request={"intent": evidence.intent.model_dump(mode="json")},
            coverage=evidence.coverage.model_dump(mode="json"),
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
            if retain_snapshot:
                await stack.enter_async_context(
                    self.wave.snapshots.hold_reference(evidence.snapshot)
                )
            async with self.database.write() as tx:
                saved = await self.results.adopt_in(tx, facts=facts, prepared_outputs=prepared)
                await self.materials.adopt_sources_in(
                    tx,
                    saved,
                    "evidence",
                    tuple(
                        MaterialSource(
                            key=part.key, material=part.material, selections=part.selections
                        )
                        for part in evidence.passages
                    ),
                )
                if retain_snapshot:
                    await self.wave.retain_in(
                        tx, evidence.snapshot, owner_kind="result", owner_id=saved.ref.id
                    )
        await call.saved(saved.ref)
        await call.events.emit(Progress(stage="evidence.save", completed=1, total=1))
        return saved.ref
