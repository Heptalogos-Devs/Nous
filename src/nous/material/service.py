"""Immutable source content and actual Import/Contact events."""

from typing import NotRequired, TypedDict

from nous_wave.contracts import (
    CallContext,
    DomainError,
    MediaReference,
    SourceRegion,
)
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import JSON, Boolean, Column, Integer, String, Table, select

from nous.platform.content import ContentStore
from nous.platform.persistence import Authority, ReadView, WriteTx, metadata
from nous.references import allocate_in

from . import admission, writes
from .access import media_source_in
from .provenance import provenance_in

materials = Table(
    "materials",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False, index=True),
    Column("content_hash", String, nullable=False, index=True),
    Column("record", JSON, nullable=False),
)
observations = Table(
    "observations",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False),
    Column("subject_id", String, nullable=False, index=True),
    Column("material_id", String, nullable=False, index=True),
    Column("kind", String, nullable=False),
    Column("active", Boolean, nullable=False, default=True),
    Column("record", JSON, nullable=False),
    Column("commit_seq", Integer, nullable=False),
)
regions = Table(
    "source_regions",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False),
    Column("material_id", String, nullable=False, index=True),
    Column("record", JSON, nullable=False),
)


class MaterialRecord(TypedDict):
    id: str
    namespace_id: str
    content_hash: str
    text: str | None
    filename: str
    media_type: str
    byte_len: int
    source_type: str
    source: NotRequired[str | None]
    origin: dict[str, str]
    event_time: dict[str, JsonValue] | None
    recorded_at: str
    commit_seq: int
    media_info: NotRequired[dict[str, JsonValue] | None]
    purged: NotRequired[bool]
    addresses: NotRequired[dict[str, JsonValue]]
    required_source_ids: NotRequired[list[str]]
    required_observation_ids: NotRequired[list[str]]


class SourceRecord(TypedDict):
    id: str
    namespace_id: str
    subject_id: str
    material_id: str
    kind: str
    origin: dict[str, str]
    source_type: str
    source: NotRequired[str | None]
    event_time: dict[str, JsonValue] | None
    observed_at: dict[str, JsonValue]
    recorded_at: str
    commit_seq: int
    purged: NotRequired[bool]
    placement: NotRequired[dict[str, JsonValue]]
    previous_observation_id: NotRequired[str]
    retry_parent_job_id: NotRequired[str]


_MATERIAL = TypeAdapter(MaterialRecord)
_SOURCE = TypeAdapter(SourceRecord)


def source_permitted(ctx: CallContext, material_id: str, observation_id: str | None = None) -> bool:
    atoms = {material_id, f"material:{material_id}"}
    if observation_id is not None:
        atoms.update((observation_id, f"observation:{observation_id}"))
    return all(bool(atoms.intersection(group)) for group in ctx.source_groups())


async def require_observation_in(
    view: ReadView | WriteTx,
    ctx: CallContext,
    observation_id: str,
    *,
    active: bool = True,
    region_id: str | None = None,
):
    row = (
        (
            await view.connection.execute(
                select(observations).where(
                    observations.c.id == observation_id,
                    observations.c.namespace_id == ctx.namespace_id,
                    observations.c.subject_id == ctx.subject_id,
                )
            )
        )
        .mappings()
        .first()
    )
    if row is None or (active and not row["active"]):
        raise DomainError(
            "source_unavailable",
            "Source is unavailable in the current read scope",
            observation_id=observation_id,
        )
    atoms = {
        str(row["material_id"]),
        f"material:{row['material_id']}",
        observation_id,
        f"observation:{observation_id}",
    }
    if region_id:
        atoms.update((region_id, f"source-region:{region_id}", f"source_region:{region_id}"))
    if not all(bool(atoms.intersection(group)) for group in ctx.source_groups()):
        raise DomainError(
            "source_unavailable",
            "Source is unavailable in the current read scope",
            observation_id=observation_id,
        )
    if region_id:
        region = (
            await view.connection.execute(
                select(regions.c.record).where(
                    regions.c.id == region_id,
                    regions.c.namespace_id == ctx.namespace_id,
                    regions.c.material_id == row["material_id"],
                )
            )
        ).scalar_one_or_none()
        if region is None:
            raise DomainError(
                "region_not_found", "Source region does not belong to the observation material"
            )
    return row


class MaterialService:
    put_in = writes.put_in
    put_blob_in = writes.put_blob_in
    observe_in = writes.observe_in
    admit_text_in = admission.admit_text_in
    admit_blob_in = admission.admit_blob_in
    admit_observation_in = admission.admit_observation_in
    restore_input_in = admission.restore_input_in

    provenance_in = provenance_in

    media_source_in = media_source_in

    def __init__(self, authority: Authority, content_store: ContentStore | None = None) -> None:
        self.authority = authority
        self.content_store = content_store

    async def register_region_in(
        self, tx: WriteTx, ctx: CallContext, region: SourceRegion
    ) -> SourceRegion:
        events = (
            (
                await tx.connection.execute(
                    select(observations.c.id).where(
                        observations.c.namespace_id == ctx.namespace_id,
                        observations.c.subject_id == ctx.subject_id,
                        observations.c.material_id == region.material_id,
                        observations.c.active.is_(True),
                    )
                )
            )
            .scalars()
            .all()
        )
        if not any(
            source_permitted(ctx, region.material_id, observation_id) for observation_id in events
        ):
            raise DomainError(
                "source_unavailable", "Creating a region requires access to its original material"
            )
        existing = (
            await tx.connection.execute(select(regions.c.record).where(regions.c.id == region.id))
        ).scalar_one_or_none()
        if existing is not None:
            stored = SourceRegion.model_validate(existing)
            if stored.model_dump(exclude={"addresses"}) != region.model_dump(exclude={"addresses"}):
                raise DomainError(
                    "region_conflict", "Source region identity already has different coordinates"
                )
            return stored
        region = region.model_copy(
            update={
                "addresses": await allocate_in(
                    tx, ctx, MediaReference(kind="source_region", id=region.id)
                )
            }
        )
        await tx.connection.execute(
            regions.insert().values(
                id=region.id,
                namespace_id=ctx.namespace_id,
                material_id=region.material_id,
                record=region.model_dump(mode="json"),
            )
        )
        return region

    async def read_region(self, ctx: CallContext, region_id: str) -> SourceRegion:
        async with self.authority.read(ctx) as view:
            return await self.read_region_in(view, ctx, region_id)

    async def read_region_in(
        self, view: ReadView | WriteTx, ctx: CallContext, region_id: str
    ) -> SourceRegion:
        value = (
            await view.connection.execute(
                select(regions.c.record).where(
                    regions.c.id == region_id, regions.c.namespace_id == ctx.namespace_id
                )
            )
        ).scalar_one_or_none()
        if value is None:
            raise DomainError("region_not_found", "Source region does not exist")
        region = SourceRegion.model_validate(value)
        events = (
            (
                await view.connection.execute(
                    select(observations.c.id).where(
                        observations.c.namespace_id == ctx.namespace_id,
                        observations.c.subject_id == ctx.subject_id,
                        observations.c.material_id == region.material_id,
                        observations.c.active.is_(True),
                    )
                )
            )
            .scalars()
            .all()
        )
        if not events:
            raise DomainError(
                "source_unavailable", "Source region has no live observation for this subject"
            )
        for observation_id in events:
            try:
                await require_observation_in(view, ctx, str(observation_id), region_id=region_id)
            except DomainError as error:
                if error.code != "source_unavailable":
                    raise
            else:
                return region
        raise DomainError(
            "source_unavailable", "No live observation grants access to this source region"
        )

    async def region_source(
        self, ctx: CallContext, region_id: str
    ) -> tuple[SourceRegion, MaterialRecord]:
        """Authorize an exact region before the media owner reads bytes for extraction.

        The returned metadata is an internal processing input. Callers expose only
        the authorized extracted region, never the containing original bytes.
        """
        region = await self.read_region(ctx, region_id)
        async with self.authority.read(ctx) as view:
            value = (
                await view.connection.execute(
                    select(materials.c.record).where(
                        materials.c.id == region.material_id,
                        materials.c.namespace_id == ctx.namespace_id,
                    )
                )
            ).scalar_one()
            return region, _MATERIAL.validate_python(value)

    async def read(self, ctx: CallContext, material_id: str) -> MaterialRecord:
        async with self.authority.read(ctx) as view:
            return await self.read_in(view, ctx, material_id)

    async def read_in(
        self, view: ReadView | WriteTx, ctx: CallContext, material_id: str
    ) -> MaterialRecord:
        from .access import read_material_in

        return await read_material_in(self, view, ctx, material_id)
