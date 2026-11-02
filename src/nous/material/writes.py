"""Material and observation writes share the caller's authority transaction."""

from __future__ import annotations

from hashlib import sha256
from typing import TYPE_CHECKING
from uuid import uuid4

from nous_sdk.contracts import (
    BlobDocument,
    CallContext,
    DomainError,
    ImportDocument,
    MediaInfo,
    MediaReference,
)
from nous_sdk.contracts.common import ObservationKind
from sqlalchemy import select

from nous.identity.access import require_namespace_in
from nous.platform.content import ContentLease
from nous.platform.persistence import ReadView, WriteTx
from nous.references import allocate_in

if TYPE_CHECKING:
    from .archive_dependencies import VerifiedHistoryArchive
    from .service import MaterialRecord, MaterialService, SourceRecord


async def put_in(
    self: MaterialService,
    tx: WriteTx,
    ctx: CallContext,
    document: ImportDocument,
    lease: ContentLease | None = None,
) -> MaterialRecord:
    from .service import materials

    encoded = document.inline_text().encode("utf-8")
    digest = sha256(encoded).hexdigest()
    if lease is not None and (lease.content_hash != digest or lease.byte_len != len(encoded)):
        raise DomainError(
            "input_content_mismatch", "Captured bytes differ from the submitted inline text"
        )
    value: MaterialRecord = {
        "id": str(uuid4()),
        "namespace_id": ctx.namespace_id,
        "content_hash": digest,
        "text": document.inline_text(),
        "filename": document.filename,
        "media_type": document.media_type,
        "byte_len": len(document.inline_text().encode("utf-8")),
        "source_type": document.source_type,
        "source": document.source,
        "origin": document.origin,
        "event_time": document.event_time.model_dump(mode="json") if document.event_time else None,
        "recorded_at": str(tx.recorded_at),
        "commit_seq": tx.commit_seq,
    }
    value["addresses"] = (
        await allocate_in(tx, ctx, MediaReference(kind="material", id=value["id"]))
    ).model_dump(mode="json")
    await tx.connection.execute(
        materials.insert().values(
            id=value["id"], namespace_id=ctx.namespace_id, content_hash=digest, record=value
        )
    )
    if lease is not None and self.content_store is not None:
        await self.content_store.attach_in(tx, lease, str(value["id"]))
    return value


async def put_blob_in(
    self: MaterialService,
    tx: WriteTx,
    ctx: CallContext,
    lease: ContentLease,
    document: BlobDocument,
    info: MediaInfo | None = None,
    *,
    required_source_ids: tuple[str, ...] = (),
    required_observation_ids: tuple[str, ...] = (),
    history_archive: VerifiedHistoryArchive | None = None,
) -> MaterialRecord:
    from .service import materials, require_observation_in

    if self.content_store is None:
        raise DomainError("content_unavailable", "Immutable content storage is not configured")
    from .access import require_dependencies_in

    if history_archive is not None:
        from .archive_dependencies import history_dependencies_in

        if history_archive.content_hash != lease.content_hash or history_archive.sources != frozenset(required_source_ids):
            raise DomainError("bundle_history_mismatch", "Archive bytes and historical dependencies differ")
        state = await history_dependencies_in(tx, ctx, required_source_ids)
        if state != "available":
            raise DomainError("source_unavailable", "Archive historical dependency is unavailable", state=state)
    elif required_source_ids:
        await require_dependencies_in(self, tx, ctx, required_source_ids)
    for observation_id in required_observation_ids:
        if required_source_ids:
            await require_observation_exists_in(tx, ctx, observation_id)
        else:
            await require_observation_in(tx, ctx, observation_id)
    value: MaterialRecord = {
        "id": str(uuid4()),
        "namespace_id": ctx.namespace_id,
        "content_hash": lease.content_hash,
        "text": None,
        "filename": document.filename,
        "media_type": lease.media_type,
        "byte_len": lease.byte_len,
        "source_type": document.source_type,
        "source": document.source,
        "required_source_ids": list(dict.fromkeys(required_source_ids)),
        "required_observation_ids": list(dict.fromkeys(required_observation_ids)),
        "origin": document.origin,
        "event_time": document.event_time.model_dump(mode="json") if document.event_time else None,
        "media_info": info.model_dump(mode="json") if info else None,
        "recorded_at": str(tx.recorded_at),
        "commit_seq": tx.commit_seq,
    }
    if history_archive is not None:
        value["dependency_purpose"] = "history"
    value["addresses"] = (
        await allocate_in(tx, ctx, MediaReference(kind="material", id=value["id"]))
    ).model_dump(mode="json")
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
    self: MaterialService,
    tx: WriteTx,
    ctx: CallContext,
    material_id: str,
    document: ImportDocument | BlobDocument,
    kind: ObservationKind = "import",
    *,
    sources: tuple[MediaReference, ...] = (),
) -> SourceRecord:
    from .service import materials, observations

    await require_namespace_in(tx, ctx)
    material = (
        await tx.connection.execute(
            select(materials.c.record).where(
                materials.c.id == material_id, materials.c.namespace_id == ctx.namespace_id
            )
        )
    ).scalar_one_or_none()
    if material is None or material.get("purged"):
        raise DomainError(
            "source_purged",
            "An unavailable or purged material cannot produce a new observation",
        )
    if sources:
        for reference in sources:
            if reference.kind in {"representation", "derived_region"}:
                from .derived import DerivedService

                info = await DerivedService(self.authority, self).source_in(tx, ctx, reference)
                matches = material_id in info.material_ids
            else:
                selected, _, _ = await self.media_source_in(tx, ctx, reference)
                matches = selected["id"] == material_id
            if not matches:
                raise DomainError(
                    "source_material_conflict", "Observation source belongs to another Material"
                )
    elif material["commit_seq"] != tx.commit_seq:
        await self.read_in(tx, ctx, material_id)
    value: SourceRecord = {
        "id": str(uuid4()),
        "namespace_id": ctx.namespace_id,
        
        "material_id": material_id,
        "kind": kind,
        "origin": document.origin,
        "source_type": document.source_type,
        "source": document.source,
        "event_time": document.event_time.model_dump(mode="json") if document.event_time else None,
        "observed_at": (document.observed_at or ctx.clock).model_dump(mode="json"),
        "recorded_at": str(tx.recorded_at),
        "commit_seq": tx.commit_seq,
    }
    if "validity" in document.model_fields_set:
        value["validity"] = (
            document.validity.model_dump(mode="json") if document.validity is not None else None
        )
    await tx.connection.execute(
        observations.insert().values(
            id=value["id"],
            namespace_id=ctx.namespace_id,
            
            material_id=material_id,
            kind=kind,
            active=True,
            record=value,
            commit_seq=tx.commit_seq,
        )
    )
    return value


async def require_observation_exists_in(
    tx: ReadView | WriteTx, ctx: CallContext, observation_id: str
) -> None:
    from .service import observations

    known = await tx.connection.scalar(
        select(observations.c.id).where(
            observations.c.id == observation_id,
            observations.c.namespace_id == ctx.namespace_id,
            
            observations.c.active.is_(True),
        )
    )
    if known is None:
        raise DomainError(
            "source_unavailable", "The selected dependency's source event is unavailable"
        )
