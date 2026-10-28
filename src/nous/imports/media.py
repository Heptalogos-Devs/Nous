"""Media formation retains exact regions and reuses original inputs in both passes."""

import asyncio
import io
from typing import Literal

from nous_wave.contracts import (
    AdoptCognition,
    AdoptMedia,
    BlobDocument,
    CallContext,
    CognitiveTimes,
    DomainError,
    EpistemicStance,
    EvidenceLink,
    FormationProposal,
    MediaEmbeddingInput,
    MemoryBody,
    SourceRegion,
)
from pydantic import TypeAdapter
from pypdf import PdfReader, PdfWriter

from nous.cognition.service import CognitionService
from nous.material.service import MaterialService
from nous.material.text import source_region_text
from nous.platform.content import ContentObject, ContentStore
from nous.platform.media import MediaService
from nous.platform.models import ModelService
from nous.platform.models.generation import Description, MediaInput
from nous.platform.persistence import Authority


def pdf_region(data: bytes, region: SourceRegion) -> bytes:
    reader = PdfReader(io.BytesIO(data))
    start = (region.page_start or 1) - 1
    end = region.page_end or len(reader.pages)
    if start < 0 or end > len(reader.pages) or start >= end:
        raise ValueError("PDF region is outside the source page range")
    writer = PdfWriter()
    for page in reader.pages[start:end]:
        writer.add_page(page)
    result = io.BytesIO()
    writer.write(result)
    return result.getvalue()


class MediaImportService:
    def __init__(
        self,
        authority: Authority,
        materials: MaterialService,
        cognition: CognitionService,
        models: ModelService,
        content: ContentStore,
        media: MediaService,
    ) -> None:
        self.authority = authority
        self.materials = materials
        self.cognition = cognition
        self.models = models
        self.content = content
        self.media = media

    async def region_input(self, ctx: CallContext, region_id: str) -> MediaInput:
        region, material = await self.materials.region_source(ctx, region_id)
        source = ContentObject(
            material["content_hash"], material["byte_len"], material["media_type"]
        )
        derived = None
        if region.pixel_rect and source.media_type.startswith("image/"):
            derived = await self.media.crop_image(source, region.pixel_rect)
        elif region.pixel_rect and source.media_type.startswith("video/"):
            derived = await self.media.crop_video(source, region.pixel_rect, region.time_span)
        elif region.time_span:
            derived = await self.media.extract_clip(
                source, region.time_span, audio_only=source.media_type.startswith("audio/")
            )
        elif region.pixel_rect:
            raise ValueError("Pixel selection requires an image or video")
        selected = derived.lease if derived else source
        try:
            if selected.byte_len > 32 * 1024 * 1024:
                raise ValueError("Selected model input exceeds 32 MiB")
            async with self.content.protect(selected) as path:
                data = await asyncio.to_thread(path.read_bytes)
            if region.page_start is not None or region.page_end is not None:
                if selected.media_type != "application/pdf":
                    raise ValueError("Page selection requires PDF material")
                data = await asyncio.to_thread(pdf_region, data, region)
            if any(
                value is not None
                for value in (
                    region.text_start,
                    region.text_end,
                    region.json_pointer,
                    region.line_number,
                )
            ):
                data = source_region_text(data.decode("utf-8"), region).encode("utf-8")
            return MediaInput(data, selected.media_type, region.id)
        finally:
            if derived:
                await self.content.release(derived.lease)

    async def resolve_embedding_input(
        self, ctx: CallContext, value: MediaEmbeddingInput
    ) -> MediaInput:
        if value.reference.kind == "source_region":
            return await self.region_input(ctx, value.reference.id)
        if value.reference.kind != "material":
            raise ValueError(
                "Embedding representation must resolve to a retained material or source region"
            )
        record = await self.materials.read(ctx, value.reference.id)
        if record["content_hash"] != value.content_hash:
            raise ValueError("Media embedding input identity differs from its retained source")
        obj = ContentObject(record["content_hash"], record["byte_len"], record["media_type"])
        if obj.byte_len > 32 * 1024 * 1024:
            raise ValueError("Media embedding input exceeds 32 MiB; select a region")
        async with self.content.protect(obj) as path:
            data = await asyncio.to_thread(path.read_bytes)
        return MediaInput(data, obj.media_type, value.reference.id)

    async def adopt(self, ctx: CallContext, command: AdoptMedia) -> dict[str, object]:
        if command.regions:
            if any(region.material_id != command.material_id for region in command.regions):
                raise ValueError("Every region must belong to the selected original")
            # New regions need whole-original authority; existing regions keep their exact scope.
            for region in command.regions:
                if ctx.allowed_source_ids is None or command.material_id in ctx.allowed_source_ids:
                    async with self.authority.write(
                        ctx,
                        f"{command.operation_id}:region:{region.id}",
                        operation_kind="material.region",
                        scope=ctx.require_subject(),
                        payload=region,
                    ) as tx:
                        if not tx.replayed:
                            tx.result(await self.materials.register_region_in(tx, ctx, region))
                elif await self.materials.read_region(ctx, region.id) != region:
                    raise ValueError("Region differs from its authorized source coordinates")
            _, material = await self.materials.region_source(ctx, command.regions[0].id)
            media = tuple([await self.region_input(ctx, region.id) for region in command.regions])
        else:
            material = await self.materials.read(ctx, command.material_id)
            if material["byte_len"] > 32 * 1024 * 1024:
                raise ValueError("Model input exceeds 32 MiB; select an exact region")
            obj = ContentObject(
                material["content_hash"], material["byte_len"], material["media_type"]
            )
            async with self.content.protect(obj) as path:
                data = await asyncio.to_thread(path.read_bytes)
            media = (MediaInput(data, obj.media_type, command.material_id),)
        strategies: dict[
            str, Literal["description_only", "direct_structured", "describe_then_structure"]
        ] = {
            "native": "direct_structured",
            "description": "description_only",
            "description_then_native": "describe_then_structure",
        }
        result = await self.models.interpret(
            ctx,
            command.instruction,
            {
                "material_id": command.material_id,
                "filename": material["filename"],
                "regions": [region.model_dump(mode="json") for region in command.regions],
            },
            media,
            strategies[command.strategy],
        )
        proposal = (
            FormationProposal(
                memories=(MemoryBody(text=result.description),),
                producer_call_id=result.producer_call_id,
            )
            if isinstance(result, Description)
            else result
        )
        document = BlobDocument(
            filename=material["filename"],
            media_type=material["media_type"],
            source_type=material["source_type"],
            origin=material["origin"],
        )
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="media.adopt",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                assert tx.receipt is not None
                return TypeAdapter(dict[str, object]).validate_python(tx.receipt.result)
            observation = await self.materials.observe_in(tx, ctx, command.material_id, document)
            evidence = tuple(
                EvidenceLink(observation_id=str(observation["id"]), region_id=region.id)
                for region in command.regions
            ) or (EvidenceLink(observation_id=str(observation["id"])),)
            if not proposal.memories:
                raise DomainError(
                    "media_empty_formation", "Media formation returned no description"
                )
            times = CognitiveTimes(
                formation=ctx.clock,
                observation=ctx.clock,
                recorded=ctx.clock,
                event=proposal.event,
                validity=proposal.validity,
            )
            anchor_body = MemoryBody(
                text="\n\n".join(item.text for item in proposal.memories),
                tags=tuple(dict.fromkeys(tag for item in proposal.memories for tag in item.tags)),
                epistemic=EpistemicStance(
                    classification="derived", attribution=material["filename"]
                ),
            )
            anchor = await self.cognition.adopt_in(
                tx,
                ctx,
                AdoptCognition(
                    body=anchor_body,
                    evidence=evidence,
                    times=times,
                    producer_call_id=proposal.producer_call_id,
                ),
            )
            semantic = await self.cognition.adopt_semantic_proposal_in(
                tx, ctx, anchor, proposal, times
            )
            records = [record.model_dump(mode="json") for record in (anchor, *semantic)]
            return tx.result(
                {
                    "observation_id": observation["id"],
                    "cognition": records,
                    "producer_call_id": proposal.producer_call_id,
                }
            )
