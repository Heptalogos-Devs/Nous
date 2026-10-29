"""Commit actual media contacts and paid derivations before cognitive adoption."""

from typing import TYPE_CHECKING, Literal

from nous_wave.contracts import (
    AdoptCognition,
    AdoptMedia,
    BlobDocument,
    CallContext,
    CognitiveTimes,
    DomainError,
    EpistemicStance,
    EvidenceLink,
    FormationDraft,
    FormationProposal,
    MediaReference,
    MemoryBody,
    TimePoint,
)
from nous_wave.contracts.cognition import ProposedMemory
from nous_wave.contracts.derivation import (
    CreateRepresentation,
    DerivationInput,
    DerivedRepresentation,
    StructuredRepresentation,
    TextRepresentation,
)
from pydantic import JsonValue, TypeAdapter

from nous.cognition.formation_validation import validate_formation_draft
from nous.material.service import MaterialRecord, SourceRecord, require_observation_in
from nous.platform.content import ContentObject
from nous.platform.persistence import ReadView, WriteTx

from .media_input import selected_input
from .model_contracts import Description
from .model_generation import interpret

if TYPE_CHECKING:
    from .media import MediaImportService


async def authorize_result(
    self: MediaImportService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    command: AdoptMedia,
    value: object,
) -> dict[str, object]:
    result = TypeAdapter(dict[str, object]).validate_python(value)
    observation_id = TypeAdapter(str).validate_python(result["observation_id"])
    references = tuple(
        MediaReference(kind="source_region", id=region.id, observation_id=observation_id)
        for region in command.regions
    ) or (MediaReference(kind="material", id=command.material_id, observation_id=observation_id),)
    for reference in references:
        await self.materials.media_source_in(view, ctx, reference)
    return result


async def observe(
    self: MediaImportService, ctx: CallContext, command: AdoptMedia, material: MaterialRecord
) -> SourceRecord:
    document = BlobDocument(
        filename=material["filename"],
        media_type=material["media_type"],
        source_type=material["source_type"],
        origin=material["origin"],
        event_time=TimePoint.model_validate(material["event_time"])
        if material["event_time"]
        else None,
    )
    async with self.authority.write(
        ctx,
        command.operation_id + ":contact",
        operation_kind="media.contact",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            return TypeAdapter(SourceRecord).validate_python(tx.replay_result)
        if command.observation_id is None:
            value = await self.materials.observe_in(tx, ctx, command.material_id, document)
        else:
            existing = await require_observation_in(
                tx,
                ctx,
                command.observation_id,
                region_id=command.regions[0].id if command.regions else None,
            )
            if existing["material_id"] != command.material_id:
                raise DomainError(
                    "observation_material_conflict", "Observation belongs to a different original"
                )
            for region in command.regions[1:]:
                await require_observation_in(tx, ctx, command.observation_id, region_id=region.id)
            value = TypeAdapter(SourceRecord).validate_python(existing["record"])
        return tx.result(value)


async def adopt(
    self: MediaImportService, ctx: CallContext, command: AdoptMedia
) -> dict[str, object]:
    completed = await self.authority.receipt(
        ctx,
        command.operation_id,
        operation_kind="media.adopt",
        scope=ctx.require_subject(),
        payload=command,
    )
    if completed is not None:
        async with self.authority.read(ctx) as view:
            return await authorize_result(self, view, ctx, command, completed.result)
    if command.regions:
        if any(region.material_id != command.material_id for region in command.regions):
            raise DomainError(
                "region_material_conflict", "Every region must belong to the selected original"
            )
        for region in command.regions:
            try:
                saved = await self.materials.read_region(ctx, region.id)
            except DomainError as error:
                if error.code != "region_not_found":
                    raise
                async with self.authority.write(
                    ctx,
                    f"{command.operation_id}:region:{region.id}",
                    operation_kind="material.region",
                    scope=ctx.require_subject(),
                    payload=region,
                ) as tx:
                    if not tx.replayed:
                        tx.result(await self.materials.register_region_in(tx, ctx, region))
            else:
                if saved.model_dump(exclude={"addresses"}) != region.model_dump(
                    exclude={"addresses"}
                ):
                    raise DomainError(
                        "region_conflict", "Region differs from its retained source coordinates"
                    )
        _, material = await self.materials.region_source(ctx, command.regions[0].id)
    else:
        material = await self.materials.read(ctx, command.material_id)
    observation = await observe(self, ctx, command, material)
    observation_id = observation["id"]
    references = tuple(
        MediaReference(kind="source_region", id=region.id, observation_id=observation_id)
        for region in command.regions
    ) or (MediaReference(kind="material", id=command.material_id, observation_id=observation_id),)
    source_inputs = tuple(
        DerivationInput(ordinal=index, source=reference)
        for index, reference in enumerate(references)
    )
    if command.regions:
        media = tuple([await self.region_input(ctx, region.id) for region in command.regions])
    else:
        obj = ContentObject(material["content_hash"], material["byte_len"], material["media_type"])
        media = (
            await selected_input(
                self.content, self.media, obj, command.material_id, inline_text=material["text"]
            ),
        )
    description_record: DerivedRepresentation | None = None
    description: Description | None = None

    async def save_description(value: Description) -> None:
        nonlocal description_record, description
        async with self.authority.read(ctx) as view:
            producer = await self.derivations.producer_for_call_in(
                view, ctx, value.producer_call_id
            )
        description_record = await self.derivations.adopt(
            ctx,
            CreateRepresentation(
                operation_id=command.operation_id + ":description",
                inputs=source_inputs,
                kind="image_description"
                if material["media_type"].startswith("image/")
                else "audio_description"
                if material["media_type"].startswith("audio/")
                else "scene_description"
                if material["media_type"].startswith("video/")
                else "resource_synopsis",
                payload=TextRepresentation(text=value.description),
                producer=producer,
            ),
        )
        description = value

    strategies: dict[
        str, Literal["description_only", "direct_structured", "describe_then_structure"]
    ] = {
        "native": "direct_structured",
        "description": "description_only",
        "description_then_native": "describe_then_structure",
    }
    source: dict[str, object] = {
        "material_id": command.material_id,
        "filename": material["filename"],
        "observation_id": observation_id,
        "source_material_ids": [command.material_id],
        "regions": [region.model_dump(mode="json") for region in command.regions],
    }
    for attempt in range(2):
        result = await interpret(
            self.models,
            ctx,
            command.instruction,
            source,
            media,
            strategies[command.strategy],
            description_adopter=save_description,
            cached_description=description,
        )
        if isinstance(result, Description):
            proposal = FormationProposal(
                memories=(ProposedMemory(text=result.description),),
                producer_call_id=result.producer_call_id,
            )
            break
        try:
            validate_formation_draft(result)
        except DomainError as error:
            if attempt or error.code not in {
                "formation_key_unknown",
                "formation_key_conflict",
                "formation_target_unknown",
                "formation_cycle",
            }:
                raise
            source = {
                **source,
                "formation_correction": {
                    "code": error.code,
                    "message": str(error),
                    **error.details,
                    "previous_call_id": result.producer_call_id,
                    "previous_proposal": result.model_dump(mode="json"),
                },
            }
        else:
            proposal = result
            break
    else:
        raise RuntimeError("Media formation exhausted its explicit correction attempts")
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="media.adopt",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            return await authorize_result(self, tx, ctx, command, tx.replay_result)
        for reference in references:
            await self.materials.media_source_in(tx, ctx, reference)
        representation = description_record
        if not isinstance(result, Description):
            if proposal.producer_call_id is None:
                raise DomainError(
                    "producer_required", "Media interpretation requires its actual model call"
                )
            producer = await self.derivations.producer_for_call_in(
                tx, ctx, proposal.producer_call_id
            )
            inputs = source_inputs
            if description_record is not None:
                inputs = (
                    *inputs,
                    DerivationInput(
                        ordinal=len(inputs),
                        source=description_record.reference.media(),
                        role="description",
                    ),
                )
            representation = await self.derivations.adopt_in(
                tx,
                ctx,
                CreateRepresentation(
                    operation_id=command.operation_id + ":structure",
                    inputs=inputs,
                    kind="structured_interpretation",
                    strategy=strategies[command.strategy],
                    payload=StructuredRepresentation(
                        schema_id="nous.formation-draft.v1",
                        value=TypeAdapter(dict[str, JsonValue]).validate_python(
                            proposal.model_dump(
                                mode="json", include=set(FormationDraft.model_fields)
                            )
                        ),
                    ),
                    producer=producer,
                ),
            )
        evidence = tuple(
            EvidenceLink(observation_id=observation_id, region_id=region.id)
            for region in command.regions
        ) or (EvidenceLink(observation_id=observation_id),)
        times = CognitiveTimes(
            formation=ctx.clock,
            observation=TimePoint.model_validate(observation["observed_at"]),
            recorded=ctx.clock,
            event=proposal.event or document_event(material),
            validity=proposal.validity,
        )
        if not proposal.memories:
            raise DomainError(
                "media_empty_formation", "Media interpretation returned no description"
            )
        anchor = await self.cognition.adopt_in(
            tx,
            ctx,
            AdoptCognition(
                body=MemoryBody(
                    media=(representation.reference.media(),) if representation else (),
                    text="\n\n".join(item.text for item in proposal.memories),
                    tags=tuple(
                        dict.fromkeys(tag for item in proposal.memories for tag in item.tags)
                    ),
                    epistemic=EpistemicStance(
                        classification="derived", attribution=material["filename"]
                    ),
                ),
                evidence=evidence,
                times=times,
                producer_call_id=proposal.producer_call_id,
            ),
        )
        semantic = await self.cognition.adopt_semantic_proposal_in(tx, ctx, anchor, proposal, times)
        return tx.result(
            {
                "observation_id": observation_id,
                "cognition": [record.model_dump(mode="json") for record in (anchor, *semantic)],
                "representation": representation.model_dump(mode="json")
                if representation
                else None,
                "description_reference": description_record.reference.media().model_dump(
                    mode="json"
                )
                if description_record
                else None,
                "producer_call_id": proposal.producer_call_id,
            }
        )


def document_event(material: MaterialRecord) -> TimePoint | None:
    return TimePoint.model_validate(material["event_time"]) if material["event_time"] else None
