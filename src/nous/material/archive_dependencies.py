"""Historical archive dependencies preserve withdrawal while enforcing live erasure and scope."""

from dataclasses import dataclass
from hashlib import sha256
from io import BytesIO
from typing import Literal
from zipfile import BadZipFile, ZipFile

from nous_sdk.contracts import CallContext, CognitiveRecord, DomainError, NamespaceBundle
from sqlalchemy import select

from nous.platform.persistence import ReadView, WriteTx

DependencyPurpose = Literal["active", "history"]
Availability = Literal["available", "withdrawn", "purged", "missing"]


@dataclass(frozen=True)
class VerifiedHistoryArchive:
    content_hash: str
    sources: frozenset[str]


def history_archive(data: bytes) -> VerifiedHistoryArchive | None:
    """Derive purpose from real validated native bytes, never uploaded record flags."""
    try:
        with ZipFile(BytesIO(data)) as archive:
            if "manifest.json" in archive.namelist():
                return None
            info = archive.getinfo("bundle.json")
            if info.file_size > 64 * 1024 * 1024:
                return None
            bundle = NamespaceBundle.model_validate_json(archive.read(info))
        if bundle.content_level != "full":
            return None
        from nous.data_management.bundle_validation import validate_graph

        validate_graph(bundle)
        return VerifiedHistoryArchive(sha256(data).hexdigest(), frozenset((
            *(record.reference.exact() for record in bundle.cognition),
            *(material.id for material in bundle.materials),
            *(observation.id for observation in bundle.observations),
        )))
    except (BadZipFile, KeyError, ValueError, DomainError):
        return None


async def history_dependencies_in(
    view: ReadView | WriteTx, ctx: CallContext, required: tuple[str, ...], *,
    authorize: bool = True, purged_observations: frozenset[str] = frozenset(),
    purged_cognition: frozenset[str] = frozenset(),
) -> Availability:
    """Read retained historical metadata; withdrawal never grants new source scope."""
    from nous.cognition.schema import revisions

    from .service import materials, observations, source_permitted

    seen: set[str] = set()

    async def event(identity: str) -> Availability:
        row = (await view.connection.execute(select(observations).where(
            observations.c.id == identity, observations.c.namespace_id == ctx.namespace_id,
            
        ))).mappings().first()
        if row is None:
            return "missing"
        if row["record"].get("purged") or identity in purged_observations:
            return "purged"
        if authorize and not source_permitted(ctx, row["material_id"], identity):
            raise DomainError("source_unavailable", "Archive history is outside the current source scope")
        return "available"

    async def visit(locator: str) -> Availability:
        if locator in seen:
            return "available"
        seen.add(locator)
        kind, colon, identity = locator.partition(":")
        if colon and "@" in identity:
            identifier, revision = identity.rsplit("@", 1)
            raw = await view.connection.scalar(select(revisions.c.record).where(
                revisions.c.namespace_id == ctx.namespace_id,
                 revisions.c.kind == kind,
                revisions.c.id == identifier, revisions.c.revision == int(revision),
            ))
            if raw is None:
                return "missing"
            record = CognitiveRecord.model_validate(raw)
            if record.purged or locator in purged_cognition or identifier in purged_cognition:
                return "purged"
            for source in record.source_roots:
                state = await event(source)
                if state != "available":
                    return state
            return "available"
        if kind == "observation":
            return await event(identity)
        material_id = identity if kind == "material" else locator
        raw = await view.connection.scalar(select(materials.c.record).where(
            materials.c.id == material_id, materials.c.namespace_id == ctx.namespace_id,
        ))
        if raw is None:
            return await event(locator)
        if raw.get("purged"):
            return "purged"
        events = (await view.connection.execute(select(observations).where(
            observations.c.material_id == material_id,
            observations.c.namespace_id == ctx.namespace_id,
            
        ))).mappings().all()
        retained = [row for row in events if not row["record"].get("purged") and row["id"] not in purged_observations]
        if not retained:
            return "purged" if events else "missing"
        if authorize and not any(source_permitted(ctx, material_id, row["id"]) for row in retained):
            raise DomainError("source_unavailable", "Archive original is outside the current source scope")
        for dependency in raw.get("required_source_ids", ()):
            state = await visit(dependency)
            if state != "available":
                return state
        return "available"

    for locator in required:
        state = await visit(locator)
        if state != "available":
            return state
    return "available"
