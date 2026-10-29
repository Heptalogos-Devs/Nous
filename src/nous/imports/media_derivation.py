"""Publish deterministic media transformations with their exact source closure."""

from hashlib import sha256
from typing import TYPE_CHECKING

from nous_wave.contracts import (
    BlobDocument,
    CallContext,
    DomainError,
    MediaReference,
    SourceRegion,
    TimeSpan,
)
from nous_wave.contracts.derivation import (
    ArtifactRepresentation,
    CreateRepresentation,
    DerivationInput,
    DerivationProducer,
    DerivedRepresentation,
    DeriveMedia,
    MediaTransformation,
)
from pydantic import TypeAdapter

from nous.platform.content import ContentLease, ContentObject
from nous.platform.media import DerivedContent, MediaService
from nous.platform.persistence.authority import canonical_json

from .media_input import selected_input

if TYPE_CHECKING:
    from .media import MediaImportService


async def source_object(
    self: MediaImportService,
    ctx: CallContext,
    reference: MediaReference,
) -> tuple[ContentObject, SourceRegion | None]:
    async with self.authority.read(ctx) as view:
        await self.derivations.source_in(view, ctx, reference)
        if reference.kind in {"material", "source_region"}:
            material, region, _ = await self.materials.media_source_in(view, ctx, reference)
            return ContentObject(
                material["content_hash"], material["byte_len"], material["media_type"]
            ), region
        selected = await self.derivations.selected_in(view, ctx, reference)
        payload = selected.payload
        if not isinstance(payload, ArtifactRepresentation):
            raise DomainError(
                "media_transform_invalid", "This transformation requires a physical media source"
            )
        s = selected.selector
        region = (
            SourceRegion(
                id=reference.id,
                material_id=payload.material_id,
                pixel_rect=s.pixel_rect,
                time_span=s.time_span,
                page_start=s.page_start,
                page_end=s.page_end,
            )
            if s
            else None
        )
        return ContentObject(payload.content_hash, payload.byte_len, payload.media_type), region


async def transform(
    media: MediaService,
    source: ContentObject,
    command: MediaTransformation,
) -> tuple[DerivedContent, ...]:
    if command.action == "crop":
        assert command.pixel_rect is not None
        if source.media_type.startswith("image/"):
            info = await media.probe(source)
            if info.frames is not None and info.frames > 1:
                return (
                    await media.animate(source, rect=command.pixel_rect, span=command.time_span),
                )
            if command.time_span is not None:
                raise DomainError("media_transform_invalid", "A still image has no time span")
            return (await media.crop_image(source, command.pixel_rect),)
        return (await media.crop_video(source, command.pixel_rect, command.time_span),)
    if command.action in {"clip", "audio_track"}:
        span = command.time_span
        if span is None:
            info = await media.probe(source)
            if not info.duration_ms:
                raise DomainError(
                    "media_transform_invalid", "Audio extraction requires temporal media"
                )
            span = TimeSpan(start_ms=0, end_ms=info.duration_ms)
        return (await media.extract_clip(source, span, audio_only=command.action == "audio_track"),)
    return await media.sample_frames(source, command.sample_at_ms, rect=command.pixel_rect)


async def derive(
    self: MediaImportService,
    ctx: CallContext,
    command: DeriveMedia,
) -> tuple[DerivedRepresentation, ...]:
    receipt = await self.authority.receipt(
        ctx,
        command.operation_id,
        operation_kind="media.derive",
        scope=ctx.require_subject(),
        payload=command,
    )
    if receipt is not None:
        values = TypeAdapter(tuple[DerivedRepresentation, ...]).validate_python(receipt.result)
        async with self.authority.read(ctx) as view:
            return tuple(
                [await self.derivations.read_in(view, ctx, value.reference) for value in values]
            )
    media = await self.media.freeze()
    source, region = await source_object(self, ctx, command.source)
    if not source.media_type.startswith(("image/", "video/", "audio/")):
        raise DomainError(
            "media_transform_invalid", "The selected operation requires image, video or audio"
        )
    prepared: ContentLease | None = None
    results: tuple[DerivedContent, ...] = ()
    try:
        if region:
            selected = await selected_input(self.content, media, source, command.source.id, region)
            prepared = await self.content.publish_bytes(selected.data, selected.media_type)
            source = prepared
        results = await transform(media, source, command.transformation)
        infos = [await media.probe(item.lease) for item in results]
        config = {
            "max_pixels": media.policy.max_pixels,
            "max_frames": media.policy.max_frames,
            "max_derivative_bytes": media.policy.max_derivative_bytes,
        }

        def digest(value: object) -> str:
            return sha256(canonical_json(value).encode()).hexdigest()

        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="media.derive",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                values = TypeAdapter(tuple[DerivedRepresentation, ...]).validate_python(
                    tx.replay_result
                )
                return tuple(
                    [await self.derivations.read_in(tx, ctx, value.reference) for value in values]
                )
            source_info = await self.derivations.source_in(tx, ctx, command.source)
            adopted: list[DerivedRepresentation] = []
            for ordinal, (item, info) in enumerate(zip(results, infos, strict=True)):
                selection = command.transformation
                if item.at_ms is not None:
                    selection = selection.model_copy(update={"sample_at_ms": (item.at_ms,)})
                producer = DerivationProducer(
                    implementation="nous.media/1",
                    config_digest=digest(config),
                    preprocessing_digest=digest(selection),
                    signature=digest(
                        {
                            "source": source.content_hash,
                            "transformation": selection,
                            "implementation": "nous.media/1",
                            "config": config,
                        }
                    ),
                )
                document = BlobDocument(
                    filename=f"{command.transformation.action}-{ordinal}.{item.lease.media_type.split('/')[-1]}",
                    media_type=item.lease.media_type,
                    source_type="derivation",
                )
                material = await self.materials.put_blob_in(
                    tx,
                    ctx,
                    item.lease,
                    document,
                    info,
                    required_source_ids=(source_info.reference.exact(),),
                    required_observation_ids=source_info.roots,
                )
                await self.materials.observe_in(tx, ctx, material["id"], document)
                adopted.append(
                    await self.derivations.adopt_in(
                        tx,
                        ctx,
                        CreateRepresentation(
                            operation_id=f"{command.operation_id}:{ordinal}",
                            inputs=(DerivationInput(ordinal=0, source=source_info.reference),),
                            kind="sampled_frame"
                            if command.transformation.action == "sample_frames"
                            else command.transformation.action,
                            strategy="preprocess",
                            transformation=selection,
                            timeline=source_info.reference,
                            producer=producer,
                            payload=ArtifactRepresentation(
                                material_id=material["id"],
                                content_hash=item.lease.content_hash,
                                byte_len=item.lease.byte_len,
                                media_type=item.lease.media_type,
                                media_info=info,
                            ),
                        ),
                    )
                )
            return tx.result(tuple(adopted))
    finally:
        for result in results:
            await self.content.release(result.lease)
        if prepared:
            await self.content.release(prepared)
