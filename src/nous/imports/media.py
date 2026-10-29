"""Authorized original and derived bytes for the common model and vector services."""

from nous_wave.contracts import CallContext, MediaEmbeddingInput, MediaReference, SourceRegion
from nous_wave.contracts.derivation import (
    ArtifactRepresentation,
    StructuredRepresentation,
    TextRepresentation,
)

from nous.cognition.service import CognitionService
from nous.material.derived import DerivedService
from nous.material.service import MaterialService
from nous.platform.content import ContentObject, ContentStore
from nous.platform.media import MediaService
from nous.platform.models import ModelService
from nous.platform.models.generation import MediaInput
from nous.platform.persistence import Authority
from nous.platform.persistence.authority import canonical_json

from .media_derivation import derive
from .media_formation import adopt
from .media_input import selected_input


class MediaImportService:
    adopt = adopt
    derive = derive

    async def generation_input(self, ctx: CallContext, reference: MediaReference) -> MediaInput:
        if reference.kind in {"representation", "derived_region"}:
            return await self.derived_input(ctx, reference)
        async with self.authority.read(ctx) as view:
            material, region, _ = await self.materials.media_source_in(view, ctx, reference)
        return await selected_input(
            self.content,
            self.media,
            ContentObject(material["content_hash"], material["byte_len"], material["media_type"]),
            reference.id,
            region,
            inline_text=material["text"],
        )

    def __init__(
        self,
        authority: Authority,
        materials: MaterialService,
        cognition: CognitionService,
        models: ModelService,
        content: ContentStore,
        media: MediaService,
        derivations: DerivedService | None = None,
    ) -> None:
        self.derivations = derivations or DerivedService(authority, materials)
        self.authority, self.materials, self.cognition = authority, materials, cognition
        self.models, self.content, self.media = models, content, media

    async def region_input(self, ctx: CallContext, region_id: str) -> MediaInput:
        region, material = await self.materials.region_source(ctx, region_id)
        return await selected_input(
            self.content,
            self.media,
            ContentObject(material["content_hash"], material["byte_len"], material["media_type"]),
            region.id,
            region,
            inline_text=material["text"],
        )

    async def derived_input(self, ctx: CallContext, reference: MediaReference) -> MediaInput:
        async with self.authority.read(ctx) as view:
            selected = await self.derivations.selected_in(view, ctx, reference)
        payload = selected.payload
        if isinstance(payload, TextRepresentation):
            return MediaInput(payload.text.encode("utf-8"), "text/plain", reference.id)
        if isinstance(payload, StructuredRepresentation):
            return MediaInput(
                canonical_json(payload.value).encode("utf-8"), "application/json", reference.id
            )
        assert isinstance(payload, ArtifactRepresentation)
        selector = selected.selector
        region = (
            SourceRegion(
                id=reference.id,
                material_id=payload.material_id,
                pixel_rect=selector.pixel_rect,
                time_span=selector.time_span,
                page_start=selector.page_start,
                page_end=selector.page_end,
            )
            if selector
            else None
        )
        return await selected_input(
            self.content,
            self.media,
            ContentObject(payload.content_hash, payload.byte_len, payload.media_type),
            reference.id,
            region,
        )

    async def resolve_embedding_input(
        self, ctx: CallContext, value: MediaEmbeddingInput
    ) -> MediaInput:
        if value.reference.kind in {"representation", "derived_region"}:
            async with self.authority.read(ctx) as view:
                record = await self.derivations.media_record_in(
                    view, ctx, value.reference, include_text=False
                )
            if (record.content_hash, record.byte_len, record.media_type) != (
                value.content_hash,
                value.byte_len,
                value.media_type,
            ):
                raise ValueError("Derived embedding input differs from its retained source")
            return await self.derived_input(ctx, value.reference)
        async with self.authority.read(ctx) as view:
            record, region, _ = await self.materials.media_source_in(view, ctx, value.reference)
        if record["content_hash"] != value.content_hash:
            raise ValueError("Media embedding input differs from its retained source")
        return await selected_input(
            self.content,
            self.media,
            ContentObject(record["content_hash"], record["byte_len"], record["media_type"]),
            value.reference.id,
            region,
            inline_text=record["text"],
        )
