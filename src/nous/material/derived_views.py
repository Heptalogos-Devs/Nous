"""Authorized lineage and query projections of immutable media representations."""

from datetime import datetime
from hashlib import sha256

from nous_sdk.contracts import (
    CallContext,
    CognitiveTimes,
    DomainError,
    MediaRecord,
    MediaReference,
    SourceRegion,
    TimePoint,
)
from nous_sdk.contracts.derivation import (
    ArtifactRepresentation,
    DerivedRegion,
    DerivedRepresentation,
    MediaLineage,
    StructuredRepresentation,
    TextRepresentation,
)
from sqlalchemy import select

from nous.platform.persistence import ReadView, WriteTx
from nous.platform.persistence.authority import canonical_json

from .derived_access import representation_ref
from .derived_regions import DerivedRegions
from .derived_schema import derived_regions, representations
from .service import observations

_UNAVAILABLE = {
    "source_unavailable",
    "representation_not_found",
    "derived_region_not_found",
    "region_not_found",
    "cognition_not_found",
    "reference_not_found",
}


class DerivedViews(DerivedRegions):
    async def media_record_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        reference: MediaReference,
        *,
        include_text: bool = True,
    ) -> MediaRecord:
        info = await self.source_in(view, ctx, reference)
        selected = await self.selected_in(view, ctx, reference)
        region = (
            await self.read_region_in(view, ctx, reference.id)
            if reference.kind == "derived_region"
            else None
        )
        parent = await self.raw_in(
            view, ctx, region.representation if region else representation_ref(reference)
        )
        payload = selected.payload
        addresses = region.addresses if region else parent.addresses
        if isinstance(payload, ArtifactRepresentation):
            digest, size, mime = payload.content_hash, payload.byte_len, payload.media_type
            text = ""
            material_id = payload.material_id
        else:
            text = (
                payload.text
                if isinstance(payload, TextRepresentation)
                else canonical_json(payload.value)
            )
            data = text.encode("utf-8")
            digest, size = sha256(data).hexdigest(), len(data)
            mime = (
                "application/json"
                if isinstance(payload, StructuredRepresentation)
                else "text/plain"
            )
            material_id = parent.material_ids[0]
        recorded = TimePoint(
            coordinate=int(datetime.fromisoformat(parent.recorded_at).timestamp() * 1000)
        )
        return MediaRecord(
            reference=reference,
            addresses=addresses,
            namespace_id=ctx.namespace_id,
            subject_id=ctx.require_subject(),
            material_id=material_id,
            content_hash=digest,
            byte_len=size,
            media_type=mime,
            filename=f"{parent.kind}-{reference.id}",
            text=text if include_text else "",
            source_roots=info.roots,
            support_roots=info.roots,
            source_regions=info.regions,
            times=CognitiveTimes(formation=parent.formed_at, recorded=recorded),
            commit_seq=max(parent.commit_seq, region.commit_seq if region else 0),
        )

    async def records_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        *,
        include_text: bool = True,
    ) -> list[MediaRecord]:
        refs = [
            DerivedRepresentation.model_validate(value).reference.media()
            for value in (
                await view.connection.execute(
                    select(representations.c.record).where(
                        representations.c.namespace_id == ctx.namespace_id,
                        representations.c.subject_id == ctx.require_subject(),
                        representations.c.head.is_(True),
                    )
                )
            ).scalars()
        ]
        refs.extend(
            MediaReference(kind="derived_region", id=str(identity))
            for identity in (
                await view.connection.execute(
                    select(derived_regions.c.id).where(
                        derived_regions.c.namespace_id == ctx.namespace_id,
                        derived_regions.c.subject_id == ctx.require_subject(),
                    )
                )
            ).scalars()
        )
        result: list[MediaRecord] = []
        for reference in refs:
            try:
                result.append(
                    await self.media_record_in(view, ctx, reference, include_text=include_text)
                )
            except DomainError as error:
                if error.code not in _UNAVAILABLE:
                    raise
        return result

    async def lineage(self, ctx: CallContext, source: MediaReference) -> MediaLineage:
        async with self.authority.read(ctx) as view:
            info = await self.source_in(view, ctx, source)
            seen: set[str] = set()
            found: dict[str, DerivedRepresentation] = {}
            selected_regions: dict[str, DerivedRegion] = {}
            originals: dict[str, SourceRegion] = {}
            unavailable: dict[str, MediaReference] = {}

            async def visit(reference: MediaReference) -> None:
                key = reference.exact()
                if key in seen:
                    return
                seen.add(key)
                try:
                    await self.source_in(view, ctx, reference)
                    if reference.kind == "representation":
                        value = await self.read_in(view, ctx, representation_ref(reference))
                        found[key] = value
                        for item in value.inputs:
                            await visit(item.source)
                    elif reference.kind == "derived_region":
                        value_region = await self.read_region_in(view, ctx, reference.id)
                        selected_regions[reference.id] = value_region
                        await visit(value_region.representation.media())
                    elif reference.kind == "source_region":
                        originals[reference.id] = await self.materials.read_region_in(
                            view, ctx, reference.id
                        )
                except DomainError as error:
                    if error.code not in _UNAVAILABLE:
                        raise
                    unavailable[key] = reference

            await visit(source)
            # An original's lineage includes its actual authorized descendants, retaining all inputs.
            if source.kind in {"material", "source_region"}:
                for raw in (
                    await view.connection.execute(
                        select(representations.c.record).where(
                            representations.c.namespace_id == ctx.namespace_id,
                            representations.c.subject_id == ctx.require_subject(),
                        )
                    )
                ).scalars():
                    value = DerivedRepresentation.model_validate(raw)
                    if value.purged or not set(value.source_roots).intersection(info.roots):
                        continue
                    if source.kind == "source_region" and not any(
                        source.id in ids for ids in value.source_regions.values()
                    ):
                        continue
                    await visit(value.reference.media())
            return MediaLineage(
                source=info.reference,
                root_material_ids=tuple(
                    sorted(
                        set(
                            (
                                await view.connection.execute(
                                    select(observations.c.material_id).where(
                                        observations.c.namespace_id == ctx.namespace_id,
                                        observations.c.subject_id == ctx.require_subject(),
                                        observations.c.id.in_(info.roots),
                                    )
                                )
                            ).scalars()
                        )
                    )
                ),
                representations=tuple(
                    sorted(found.values(), key=lambda r: (r.commit_seq, r.reference.id))
                ),
                derived_regions=tuple(selected_regions.values()),
                source_regions=tuple(originals.values()),
                unavailable_inputs=tuple(unavailable.values()),
                watermark=view.commit_seq,
            )
