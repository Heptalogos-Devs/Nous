"""Authorize exact derived outputs without granting their containing originals."""

from dataclasses import dataclass

from nous_wave.contracts import CallContext, DomainError, MediaInfo, MediaReference
from nous_wave.contracts.derivation import (
    ArtifactRepresentation,
    ChannelAccess,
    DerivedRegion,
    DerivedRepresentation,
    RepresentationRef,
)
from sqlalchemy import select

from nous.platform.persistence import Authority, ReadView, WriteTx

from .derived_schema import derived_regions, representations
from .service import MaterialService


def representation_ref(reference: MediaReference) -> RepresentationRef:
    if reference.kind != "representation" or "@" not in reference.id:
        raise DomainError(
            "representation_reference_invalid", "An exact representation requires id@revision"
        )
    identity, revision = reference.id.rsplit("@", 1)
    return RepresentationRef(id=identity, revision=int(revision))


def granted(ctx: CallContext, reference: MediaReference) -> bool:
    return (
        ctx.allowed_source_ids is None
        or reference.id in ctx.allowed_source_ids
        or reference.exact() in ctx.allowed_source_ids
    )


def selections(ctx: CallContext) -> tuple[CallContext, ...]:
    return tuple(ctx.for_source_group(group) for group in ctx.source_groups()) or (ctx,)


def merge_access(values: tuple[ChannelAccess, ...]) -> ChannelAccess:
    order = {"unavailable": 0, "representation": 1, "original": 2}
    return ChannelAccess.model_validate(
        {
            axis: max(
                (getattr(value, axis) for value in values),
                key=order.__getitem__,
                default="unavailable",
            )
            for axis in ("visual", "audio", "source_text")
        }
    )


def represented(record: DerivedRepresentation) -> ChannelAccess:
    if isinstance(record.payload, ArtifactRepresentation):
        return record.access
    return ChannelAccess.model_validate(
        {
            axis: "unavailable"
            if getattr(record.access, axis) == "unavailable"
            else "representation"
            for axis in ("visual", "audio", "source_text")
        }
    )


@dataclass(frozen=True)
class SourceInfo:
    reference: MediaReference
    roots: tuple[str, ...]
    material_ids: tuple[str, ...]
    access: ChannelAccess
    regions: dict[str, tuple[str, ...]]


class DerivedAccess:
    authority: Authority
    materials: MaterialService

    async def raw_in(
        self, view: ReadView | WriteTx, ctx: CallContext, reference: RepresentationRef
    ) -> DerivedRepresentation:
        value = await view.connection.scalar(
            select(representations.c.record).where(
                representations.c.namespace_id == ctx.namespace_id,
                representations.c.subject_id == ctx.require_subject(),
                representations.c.id == reference.id,
                representations.c.revision == reference.revision,
            )
        )
        if value is None:
            raise DomainError("representation_not_found", "Derived representation is unavailable")
        result = DerivedRepresentation.model_validate(value)
        if result.purged or result.payload is None:
            raise DomainError("source_unavailable", "Derived representation was purged")
        return result

    async def live_in(
        self, view: ReadView | WriteTx, ctx: CallContext, value: DerivedRepresentation
    ) -> None:
        from .derived_availability import dependency_availability_in

        state = await dependency_availability_in(view, ctx, value.reference.media())
        if state != "available":
            raise DomainError(
                "source_unavailable", "A required derivation source is unavailable", state=state
            )

    async def read_in(
        self, view: ReadView | WriteTx, ctx: CallContext, reference: RepresentationRef
    ) -> DerivedRepresentation:
        result = await self.raw_in(view, ctx, reference)
        await self.live_in(view, ctx, result)
        for selected in selections(ctx):
            if not granted(selected, reference.media()):
                for item in result.inputs:
                    await self.source_in(view, selected, item.source)
        return result

    async def read(self, ctx: CallContext, reference: RepresentationRef) -> DerivedRepresentation:
        async with self.authority.read(ctx) as view:
            return await self.read_in(view, ctx, reference)

    async def read_region_in(
        self, view: ReadView | WriteTx, ctx: CallContext, region_id: str
    ) -> DerivedRegion:
        value = await view.connection.scalar(
            select(derived_regions.c.record).where(
                derived_regions.c.namespace_id == ctx.namespace_id,
                derived_regions.c.subject_id == ctx.require_subject(),
                derived_regions.c.id == region_id,
            )
        )
        if value is None:
            raise DomainError("derived_region_not_found", "Derived region is unavailable")
        region = DerivedRegion.model_validate(value)
        if region.purged:
            raise DomainError("source_unavailable", "Derived region was purged")
        parent = await self.raw_in(view, ctx, region.representation)
        await self.live_in(view, ctx, parent)
        for selected in selections(ctx):
            if not granted(selected, MediaReference(kind="derived_region", id=region_id)):
                await self.read_in(view, selected, region.representation)
        return region

    async def read_region(self, ctx: CallContext, region_id: str) -> DerivedRegion:
        async with self.authority.read(ctx) as view:
            return await self.read_region_in(view, ctx, region_id)

    async def source_in(
        self, view: ReadView | WriteTx, ctx: CallContext, reference: MediaReference
    ) -> SourceInfo:
        if reference.kind in {"material", "source_region"}:
            material, region, observation_id = await self.materials.media_source_in(
                view, ctx, reference
            )
            provenance = await self.materials.provenance_in(view, ctx, reference)
            details = material.get("media_info")
            info = MediaInfo.model_validate(details) if details else None
            media_type = material["media_type"]
            access = ChannelAccess(
                visual="original" if media_type.startswith(("image/", "video/")) else "unavailable",
                audio="original"
                if media_type.startswith("audio/")
                or media_type.startswith("video/")
                and info is not None
                and bool(info.channels)
                else "unavailable",
                source_text="original"
                if material.get("text") is not None
                or media_type.startswith("text/")
                or media_type == "application/pdf"
                else "unavailable",
            )
            return SourceInfo(
                reference.model_copy(update={"observation_id": observation_id}),
                provenance.roots,
                provenance.material_ids,
                access,
                provenance.regions,
            )
        if reference.kind == "representation":
            value = await self.read_in(view, ctx, representation_ref(reference))
        else:
            region = await self.read_region_in(view, ctx, reference.id)
            value = await self.raw_in(view, ctx, region.representation)
        if reference.observation_id and reference.observation_id not in value.source_roots:
            raise DomainError(
                "source_unavailable", "Observation is not an input of this derivation"
            )
        return SourceInfo(
            reference,
            value.source_roots,
            value.material_ids,
            represented(value),
            value.source_regions,
        )

    async def authorize_in(
        self, view: ReadView | WriteTx, ctx: CallContext, reference: MediaReference
    ) -> None:
        await self.source_in(view, ctx, reference)
