"""Context compilation owned behavior."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    AssociationBody,
    CognitiveRecord,
    ConcernBody,
    DesireBody,
    EntityBody,
    EpistemicStance,
    JournalBody,
    MemoryBody,
    MemorySemanticsBody,
    NarrativeThreadBody,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.context import (
    CognitiveTarget,
    ContextProjection,
    ContextSegment,
    ContextTarget,
    MediaTarget,
)
from nous_sdk.presentation import body_text
from pydantic import JsonValue, TypeAdapter

from nous.cognition.entities import project_entity_in
from nous.cognition.projection import evidence_in
from nous.platform.persistence import ReadView, WriteTx
from nous.platform.persistence.authority import canonical_json

if TYPE_CHECKING:
    pass


class Projection:
    async def require_targets_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        targets: tuple[ContextTarget, ...],
    ) -> None:
        for target in targets:
            if isinstance(target, CognitiveTarget):
                await self.cognition.read_in(view, ctx, target.reference)
            elif target.reference.kind == "material":
                await self.materials.read_in(view, ctx, target.reference.id)
            elif target.reference.kind == "source_region":
                await self.materials.read_region_in(view, ctx, target.reference.id)
            else:
                await self.derivations.authorize_in(view, ctx, target.reference)

    async def require_projection_in(
        self, view: ReadView | WriteTx, ctx: CallContext, projection: ContextProjection
    ) -> None:
        await self.require_targets_in(
            view,
            ctx,
            tuple(
                target
                for segment in projection.segments
                for target in (*segment.sources, *segment.dependencies)
            ),
        )
        for segment in projection.segments:
            if segment.authority != "recorded_cognition":
                continue
            for target in segment.sources:
                if (
                    not isinstance(target, CognitiveTarget)
                    or target.reference.kind != "entity"
                    or target.reference.exact() != segment.key
                ):
                    continue
                current = await self.cognition.read_in(view, ctx, target.reference)
                expected = await self.record_segment_in(view, ctx, current)
                if expected.text != segment.text or expected.dependencies != segment.dependencies:
                    raise DomainError(
                        "context_source_changed",
                        "The Entity's supported fields changed; prepare the context again",
                    )

    async def record_segment_in(
        self, view: ReadView | WriteTx, ctx: CallContext, record: CognitiveRecord
    ) -> ContextSegment:
        record = await project_entity_in(self.cognition, view, ctx, record)
        if record.body is None:
            raise DomainError("source_purged", "Purged cognitive content cannot enter a context")
        if isinstance(record.body, (MemoryBody, MemorySemanticsBody, AssociationBody)):
            epistemic = record.body.epistemic
        elif isinstance(record.body, JournalBody):
            epistemic = record.body.perspective
        elif isinstance(record.body, NarrativeThreadBody):
            epistemic = EpistemicStance(
                classification="narrative", attribution=record.reference.exact()
            )
        else:
            epistemic = None
        segment = ContextSegment(
            key=record.reference.exact(),
            text=canonical_json(record.body)
            if isinstance(record.body, EntityBody)
            else body_text(record.body),
            sources=(CognitiveTarget(reference=record.reference),),
            dependencies=tuple(
                dict.fromkeys(
                    CognitiveTarget(reference=edge.reference)
                    for edge in (*record.evidence, *tuple(evidence_in(record.body)))
                    if edge.reference is not None
                )
            ),
            authority="recorded_cognition",
            provenance={
                "source_roots": list(record.source_roots),
                "revision": record.reference.revision,
            },
            epistemic=epistemic,
        )
        if isinstance(record.body, (DesireBody, ConcernBody)):
            saved = await self.motivation.latest_motivation_assessment_in(
                view, ctx, record.reference
            )
            if saved is not None:
                evidence = tuple(CognitiveTarget(reference=ref) for ref in saved.references)
                segment = segment.model_copy(
                    update={
                        "text": segment.text
                        + "\n"
                        + canonical_json({"motivation_assessment": saved.assessment}),
                        "sources": tuple(dict.fromkeys((*segment.sources, *evidence))),
                        "dependencies": tuple(dict.fromkeys((*segment.dependencies, *evidence))),
                        "provenance": {
                            **segment.provenance,
                            "motivation_assessment_id": saved.id,
                            "motivation_assessment_commit_seq": saved.commit_seq,
                        },
                    }
                )
        return segment

    async def media_segment_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        target: MediaTarget,
        *,
        required: bool = False,
    ) -> ContextSegment:
        from nous_sdk.contracts.derivation import StructuredRepresentation, TextRepresentation

        await self.require_targets_in(view, ctx, (target,))
        reference = target.reference
        if reference.kind in {"material", "source_region"}:
            material, region, observation_id = await self.materials.media_source_in(
                view, ctx, reference
            )
            text = await self.materials.text_in(view, ctx, reference)
            provenance = {
                "media_reference": reference.model_dump(mode="json"),
                "source_roots": [observation_id],
                "media_type": material["media_type"],
            }
            epistemic = None
        else:
            selected = await self.derivations.selected_in(view, ctx, reference)
            info = await self.derivations.source_in(view, ctx, reference)
            if isinstance(selected.payload, TextRepresentation):
                text = selected.payload.text
            elif isinstance(selected.payload, StructuredRepresentation):
                text = canonical_json(selected.payload.value)
            else:
                text = canonical_json(selected.payload)
            provenance = {
                "media_reference": reference.model_dump(mode="json"),
                "source_roots": list(info.roots),
                "source_channel_access": info.access.model_dump(mode="json"),
            }
            epistemic = EpistemicStance(classification="derived", attribution=reference.exact())
        return ContextSegment(
            key=reference.exact(),
            text=text or reference.exact(),
            sources=(target,),
            required=required,
            authority="source_material",
            provenance=TypeAdapter(dict[str, JsonValue]).validate_python(provenance),
            epistemic=epistemic,
        )
