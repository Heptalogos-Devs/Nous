"""Commit model-derived media representations before cognitive adoption."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    AdoptMedia,
    BlobDocument,
    CallContext,
    CognitiveRecord,
    DomainError,
    EvidenceLink,
    MediaReference,
    TimePoint,
)
from nous_sdk.contracts.derivation import (
    CreateRepresentation,
    DerivationInput,
    DerivedRepresentation,
    RepresentationRef,
    TextRepresentation,
)
from nous.material.observation import cognitive_times
from nous.material.service import MaterialRecord, SourceRecord
from nous.platform.content import ContentObject
from nous.platform.persistence import ReadView, WriteTx

from .media_input import selected_input
from .model_generation import describe, extract_graph, form_notes, resolve_graph

if TYPE_CHECKING:
    from .media import MediaImportService


async def accepted_media_in(
    self: MediaImportService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    command: AdoptMedia,
    observation_id: str,
) -> SourceRecord:
    references = (
        (command.source,)
        if command.source is not None
        else tuple(
            MediaReference(kind="source_region", id=region.id, observation_id=observation_id)
            for region in command.regions
        )
        or (MediaReference(kind="material", id=command.material_id, observation_id=observation_id),)
    )
    event: SourceRecord | None = None
    for reference in references:
        selected = await self.materials.accepted_source_in(view, ctx, observation_id, reference)
        if selected.material["id"] != command.material_id:
            raise DomainError(
                "observation_material_conflict", "Observation belongs to a different original"
            )
        event = selected.event
    assert event is not None
    return event


async def observe(
    self: MediaImportService, ctx: CallContext, command: AdoptMedia, material: MaterialRecord
) -> tuple[SourceRecord, CallContext]:
    document = BlobDocument(
        filename=material["filename"],
        media_type=material["media_type"],
        source_type=material["source_type"],
        origin=material["origin"],
        event_time=TimePoint.model_validate(material["event_time"])
        if material["event_time"]
        else None,
    )
    async with self.authority.write(ctx) as tx:
        if command.observation_id is None:
            sources = tuple(
                MediaReference(kind="source_region", id=region.id) for region in command.regions
            ) or (MediaReference(kind="material", id=command.material_id),)
            admission = await self.materials.admit_observation_in(
                tx, ctx, command.material_id, document, sources=sources
            )
            value, effective = admission.event, admission.context
        else:
            value = await accepted_media_in(self, tx, ctx, command, command.observation_id)
            effective = ctx
        return value, effective


def description_kind(media_type: str) -> str:
    return (
        "image_description"
        if media_type.startswith("image/")
        else "audio_description"
        if media_type.startswith("audio/")
        else "scene_description"
        if media_type.startswith("video/")
        else "resource_synopsis"
    )


async def cached_description(
    self: MediaImportService,
    ctx: CallContext,
    inputs: tuple[DerivationInput, ...],
    kind: str,
) -> DerivedRepresentation | None:
    identities = tuple(item.source.exact() for item in inputs)
    async with self.authority.read(ctx) as view:
        material_ids: set[str] = set()
        for item in inputs:
            info = await self.derivations.source_in(view, ctx, item.source)
            material_ids.update(info.material_ids)
        for material_id in sorted(material_ids):
            for representation in await self.derivations.for_material_in(view, ctx, material_id):
                if (
                    representation.kind == kind
                    and isinstance(representation.payload, TextRepresentation)
                    and tuple(item.source.exact() for item in representation.inputs) == identities
                ):
                    return representation
    return None


async def adopt(
    self: MediaImportService, ctx: CallContext, command: AdoptMedia
) -> dict[str, object]:
    if command.source is not None:
        assert command.observation_id is not None
        async with self.authority.read(ctx) as view:
            selected = await self.materials.accepted_source_in(
                view, ctx, command.observation_id, command.source
            )
            material = selected.material
        if material["id"] != command.material_id:
            raise DomainError(
                "observation_material_conflict", "Selected source belongs to a different original"
            )
    elif command.regions:
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
                async with self.authority.write(ctx) as tx:
                    await self.materials.register_region_in(tx, ctx, region)
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
    observation, ctx = await observe(self, ctx, command, material)
    observation_id = observation["id"]
    contact_source = observation.get("contact_source")
    references = (
        (MediaReference.model_validate(contact_source),)
        if contact_source is not None
        else (command.source,)
        if command.source is not None
        else tuple(
            MediaReference(kind="source_region", id=region.id, observation_id=observation_id)
            for region in command.regions
        )
        or (MediaReference(kind="material", id=command.material_id, observation_id=observation_id),)
    )
    source_inputs = tuple(
        DerivationInput(ordinal=index, source=reference)
        for index, reference in enumerate(references)
    )
    source_ids = tuple(reference.exact() for reference in references)
    prepared_media = await self.media.freeze()
    if command.source is not None:
        media = (await self.generation_input(ctx, references[0], media=prepared_media),)
    elif command.regions:
        media = tuple(
            await self.region_input(ctx, region.id, media=prepared_media)
            for region in command.regions
        )
    else:
        obj = ContentObject(material["content_hash"], material["byte_len"], material["media_type"])
        media = (
            await selected_input(
                self.content, prepared_media, obj, command.material_id, inline_text=material["text"]
            ),
        )

    descriptions: list[DerivedRepresentation] = []
    description_text: str | None = None
    needs_description = command.media_strategy in {
        "description_only",
        "describe_then_structure",
    }
    if needs_description:
        kind = description_kind(media[0].media_type)
        description_record = await cached_description(self, ctx, source_inputs, kind)
        if description_record is None:
            description_result = await describe(
                self.models,
                ctx,
                material["text"] or "",
                background=command.background,
                source_ids=source_ids,
                media=media,
            )
            description_text = description_result.output
            description_record = await self.derivations.adopt(
                ctx,
                CreateRepresentation(
                    operation_id=command.operation_id + ":description",
                    inputs=source_inputs,
                    kind=kind,
                    strategy="description_only",
                    payload=TextRepresentation(text=description_result.output),
                    producer=description_result.producer,
                ),
            )
        else:
            if not isinstance(description_record.payload, TextRepresentation):
                raise DomainError("description_result_invalid", "Saved description has no readable text")
            description_text = description_record.payload.text
        descriptions.append(description_record)

    if command.media_strategy == "description_only":
        return {
            "observation_id": observation_id,
            "cognition": [],
            "representations": [item.reference.model_dump(mode="json") for item in descriptions],
            "description": description_text,
        }

    semantic_text = (
        description_text
        if command.media_strategy == "describe_then_structure"
        else material["text"]
    ) or ""
    semantic_inputs = source_inputs
    if command.media_strategy == "describe_then_structure":
        assert descriptions
        semantic_inputs = (
            *source_inputs,
            DerivationInput(
                ordinal=len(source_inputs),
                source=descriptions[0].reference.media(),
                role="description",
            ),
        )
    notes_result = None
    graph_result = None
    graph_plan = None
    graph_resolution = None
    if command.formation_purpose in {"memory", "memory_and_graph"}:
        notes_result = await form_notes(
            self.models,
            ctx,
            semantic_text,
            configuration=self.configuration,
            background=command.background,
            source_ids=source_ids,
            media=media,
        )
    if command.formation_purpose in {"graph", "memory_and_graph"}:
        graph_result = await extract_graph(
            self.models,
            ctx,
            semantic_text,
            configuration=self.configuration,
            background=command.background,
            source_ids=source_ids,
            media=media,
        )
        async with self.authority.read(ctx) as view:
            catalog = await self.formation.formation_catalog_in(view, ctx, semantic_text)
        graph_plan = self.formation.graph_resolution_plan(
            graph_result.output, semantic_text, catalog
        )
        if graph_plan.request is not None:
            graph_resolution = await resolve_graph(
                self.models, ctx, graph_plan.request, source_ids=source_ids
            )

    evidence = tuple(
        dict.fromkeys(
            EvidenceLink(
                observation_id=observation_id,
                region_id=reference.id if reference.kind == "source_region" else None,
            )
            for reference in references
        )
    )
    if not evidence:
        evidence = (EvidenceLink(observation_id=observation_id),)
    async with self.authority.write(ctx) as tx:
        await accepted_media_in(self, tx, ctx, command, observation_id)
        for reference in references:
            if reference.kind in {"representation", "derived_region"}:
                await self.derivations.source_in(tx, ctx, reference)
            else:
                await self.materials.media_source_in(tx, ctx, reference)
        times = cognitive_times(
            observation,
            ctx.clock,
            inferred_event=document_event(material),
        )
        records: list[CognitiveRecord] = []
        representations: list[RepresentationRef] = [
            item.reference for item in descriptions
        ]
        if notes_result is not None:
            adopted, representation = await self.formation.adopt_notes_result_in(
                tx,
                ctx,
                notes_result,
                observation_id=observation_id,
                region_id=references[0].id if references[0].kind == "source_region" else None,
                times=times,
                inputs=semantic_inputs,
                evidence=evidence,
                media=references,
                strategy=command.media_strategy,
            )
            records.extend(adopted)
            representations.append(representation)
        if graph_result is not None and graph_plan is not None:
            adopted, graph_representations = await self.formation.adopt_graph_result_in(
                tx,
                ctx,
                graph_result,
                resolution=graph_resolution,
                plan=graph_plan,
                observation_id=observation_id,
                times=times,
                inputs=semantic_inputs,
                evidence=evidence,
                media=references,
                strategy=command.media_strategy,
            )
            records.extend(adopted)
            representations.extend(graph_representations)
        return {
            "observation_id": observation_id,
            "cognition": [record.model_dump(mode="json") for record in records],
            "representations": [item.model_dump(mode="json") for item in representations],
            "description": description_text,
        }


def document_event(material: MaterialRecord) -> TimePoint | None:
    return TimePoint.model_validate(material["event_time"]) if material["event_time"] else None
