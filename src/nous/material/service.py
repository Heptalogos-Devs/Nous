"""Immutable source content and actual Import/Contact events."""

from hashlib import sha256
from typing import NotRequired, TypedDict
from uuid import uuid4

from nous_wave.contracts import (
    BlobDocument,
    CallContext,
    DomainError,
    ImportDocument,
    MediaInfo,
    SourceRegion,
)
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import JSON, Boolean, Column, Integer, String, Table, select

from nous.identity.service import require_subject_in
from nous.platform.content import ContentLease, ContentStore
from nous.platform.persistence import Authority, ReadView, WriteTx, metadata

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
    origin: dict[str, str]
    event_time: dict[str, JsonValue] | None
    recorded_at: str
    commit_seq: int
    media_info: NotRequired[dict[str, JsonValue] | None]
    purged: NotRequired[bool]


class SourceRecord(TypedDict):
    id: str
    namespace_id: str
    subject_id: str
    material_id: str
    kind: str
    origin: dict[str, str]
    source_type: str
    event_time: dict[str, JsonValue] | None
    observed_at: dict[str, JsonValue]
    recorded_at: str
    commit_seq: int
    purged: NotRequired[bool]


_MATERIAL = TypeAdapter(MaterialRecord)
_SOURCE = TypeAdapter(SourceRecord)


def source_permitted(ctx: CallContext, material_id: str, observation_id: str | None = None) -> bool:
    return (
        ctx.allowed_source_ids is None
        or material_id in ctx.allowed_source_ids
        or observation_id in ctx.allowed_source_ids
    )


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
    if (
        row is None
        or (active and not row["active"])
        or (
            not source_permitted(ctx, row["material_id"], observation_id)
            and not (
                region_id
                and ctx.allowed_source_ids is not None
                and region_id in ctx.allowed_source_ids
            )
        )
    ):
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
            if SourceRegion.model_validate(existing) != region:
                raise DomainError(
                    "region_conflict", "Source region identity already has different coordinates"
                )
            return region
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
            await require_observation_in(view, ctx, str(events[0]), region_id=region_id)
            return region

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

    async def put_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        document: ImportDocument,
        lease: ContentLease | None = None,
    ) -> MaterialRecord:
        digest = sha256(document.inline_text().encode("utf-8")).hexdigest()
        row = (
            await tx.connection.execute(
                select(materials.c.record).where(
                    materials.c.namespace_id == ctx.namespace_id, materials.c.content_hash == digest
                )
            )
        ).first()
        if row and not row[0].get("purged"):
            if lease is not None and self.content_store is not None:
                await self.content_store.attach_in(tx, lease, row[0]["id"])
            return _MATERIAL.validate_python(row[0])
        value: MaterialRecord = {
            "id": str(uuid4()),
            "namespace_id": ctx.namespace_id,
            "content_hash": digest,
            "text": document.inline_text(),
            "filename": document.filename,
            "media_type": document.media_type,
            "byte_len": len(document.inline_text().encode("utf-8")),
            "source_type": document.source_type,
            "origin": document.origin,
            "event_time": document.event_time.model_dump(mode="json")
            if document.event_time
            else None,
            "recorded_at": str(tx.recorded_at),
            "commit_seq": tx.commit_seq,
        }
        await tx.connection.execute(
            materials.insert().values(
                id=value["id"], namespace_id=ctx.namespace_id, content_hash=digest, record=value
            )
        )
        if lease is not None and self.content_store is not None:
            await self.content_store.attach_in(tx, lease, str(value["id"]))
        return value

    async def put_blob_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        lease: ContentLease,
        document: BlobDocument,
        info: MediaInfo | None = None,
    ) -> MaterialRecord:
        if self.content_store is None:
            raise DomainError("content_unavailable", "Immutable content storage is not configured")
        existing = (
            await tx.connection.execute(
                select(materials.c.record).where(
                    materials.c.namespace_id == ctx.namespace_id,
                    materials.c.content_hash == lease.content_hash,
                )
            )
        ).scalar_one_or_none()
        if existing is not None and not existing.get("purged"):
            await self.content_store.attach_in(tx, lease, existing["id"])
            return _MATERIAL.validate_python(existing)
        value: MaterialRecord = {
            "id": str(uuid4()),
            "namespace_id": ctx.namespace_id,
            "content_hash": lease.content_hash,
            "text": None,
            "filename": document.filename,
            "media_type": lease.media_type,
            "byte_len": lease.byte_len,
            "source_type": document.source_type,
            "origin": document.origin,
            "event_time": document.event_time.model_dump(mode="json")
            if document.event_time
            else None,
            "media_info": info.model_dump(mode="json") if info else None,
            "recorded_at": str(tx.recorded_at),
            "commit_seq": tx.commit_seq,
        }
        await tx.connection.execute(
            materials.insert().values(
                id=value["id"],
                namespace_id=ctx.namespace_id,
                content_hash=lease.content_hash,
                record=value,
            )
        )
        await self.content_store.attach_in(tx, lease, value["id"])
        return value

    async def observe_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        material_id: str,
        document: ImportDocument | BlobDocument,
        kind: str = "import",
    ) -> SourceRecord:
        await require_subject_in(tx, ctx)
        material = (await tx.connection.execute(select(materials.c.record).where(materials.c.id == material_id, materials.c.namespace_id == ctx.namespace_id))).scalar_one_or_none()
        if material is None or material.get("purged"):
            raise DomainError("source_purged", "An unavailable or purged material cannot produce a new observation")
        value: SourceRecord = {
            "id": str(uuid4()),
            "namespace_id": ctx.namespace_id,
            "subject_id": ctx.require_subject(),
            "material_id": material_id,
            "kind": kind,
            "origin": document.origin,
            "source_type": document.source_type,
            "event_time": document.event_time.model_dump(mode="json")
            if document.event_time
            else None,
            "observed_at": (document.observed_at or ctx.clock).model_dump(mode="json"),
            "recorded_at": str(tx.recorded_at),
            "commit_seq": tx.commit_seq,
        }
        await tx.connection.execute(
            observations.insert().values(
                id=value["id"],
                namespace_id=ctx.namespace_id,
                subject_id=ctx.require_subject(),
                material_id=material_id,
                kind=kind,
                active=True,
                record=value,
                commit_seq=tx.commit_seq,
            )
        )
        return value

    async def read(self, ctx: CallContext, material_id: str) -> MaterialRecord:
        async with self.authority.read(ctx) as view:
            return await self.read_in(view, ctx, material_id)

    async def read_in(self, view: ReadView | WriteTx, ctx: CallContext, material_id: str) -> MaterialRecord:
        await require_subject_in(view, ctx)
        events = (
            (
                await view.connection.execute(
                    select(observations).where(
                        observations.c.material_id == material_id,
                        observations.c.subject_id == ctx.subject_id,
                        observations.c.namespace_id == ctx.namespace_id,
                        observations.c.active.is_(True),
                    )
                )
            )
            .mappings()
            .all()
        )
        if not any(source_permitted(ctx, material_id, row["id"]) for row in events):
            raise DomainError(
                "source_unavailable", "Source is unavailable in the current read scope"
            )
        value = (
            await view.connection.execute(
                select(materials.c.record).where(
                    materials.c.id == material_id, materials.c.namespace_id == ctx.namespace_id
                )
            )
        ).scalar_one()
        return _MATERIAL.validate_python(value)
