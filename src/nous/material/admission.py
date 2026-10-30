"""Trusted new input grants are bounded to freshly written identities and this operation."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    BlobDocument,
    CallContext,
    DomainError,
    ImportDocument,
    MediaInfo,
    MediaReference,
)
from sqlalchemy import JSON, Boolean, Column, Integer, String, Table, select

from nous.platform.content import ContentLease
from nous.platform.persistence import ReadView, WriteTx, metadata
from nous.platform.persistence.authority import operation_key

if TYPE_CHECKING:
    from .service import MaterialRecord, MaterialService, SourceRecord


input_admissions = Table(
    "material_input_admissions",
    metadata,
    Column("observation_id", String, primary_key=True),
    Column("material_id", String, nullable=False),
    Column("new_material", Boolean, nullable=False),
    Column("source_references", JSON, nullable=False),
    Column("namespace_id", String, nullable=False),
    Column("subject_id", String, nullable=False),
    Column("caller_id", String, nullable=False),
    Column("operation_key", String, nullable=False, index=True),
    Column("commit_seq", Integer, nullable=False),
)


@dataclass(frozen=True)
class InputAdmission:
    material: MaterialRecord
    event: SourceRecord
    context: CallContext


@dataclass(frozen=True)
class RestoredInputAdmission:
    context: CallContext
    commit_seq: int
    material_ids: tuple[str, ...]
    observation_ids: tuple[str, ...]


def _context_for_input(
    ctx: CallContext,
    material_id: str,
    event_id: str,
    *,
    new_material: bool = True,
    region_ids: tuple[str, ...] = (),
) -> CallContext:
    # New UUIDs may broaden only the request's temporary selection. The credential
    # ceiling is independently ANDed by CallContext.source_groups and is never edited.
    fresh = ((material_id, "material:" + material_id) if new_material else ()) + (
        event_id,
        "observation:" + event_id,
    )
    constraints = ctx.source_constraints or (
        () if ctx.allowed_source_ids is None else (ctx.allowed_source_ids,)
    )
    qualifier = tuple(
        atom
        for region in region_ids
        for atom in (region, "source-region:" + region, "source_region:" + region)
    )
    alternatives = (fresh, qualifier) if qualifier else (fresh,)
    expanded = tuple(
        tuple(dict.fromkeys((*group, *atoms))) for group in constraints for atoms in alternatives
    )
    effective = ctx.model_copy(
        update={
            "source_constraints": expanded,
            "allowed_source_ids": tuple(dict.fromkeys((*(ctx.allowed_source_ids or ()), *fresh)))
            if constraints
            else None,
        }
    )
    from .service import source_permitted

    permitted = (
        source_permitted(effective, material_id, event_id)
        if not qualifier
        else all(
            bool(
                {
                    material_id,
                    "material:" + material_id,
                    event_id,
                    "observation:" + event_id,
                    *qualifier,
                }.intersection(group)
            )
            for group in effective.source_groups()
        )
    )
    if not permitted:
        raise DomainError(
            "source_scope_forbidden",
            "The credential source ceiling does not permit the submitted input",
        )
    return effective


def _require_transaction(tx: WriteTx, ctx: CallContext) -> None:
    if (ctx.caller_id, ctx.namespace_id, ctx.subject_id, ctx.credential_source_ids) != (
        tx.ctx.caller_id,
        tx.ctx.namespace_id,
        tx.ctx.subject_id,
        tx.ctx.credential_source_ids,
    ):
        raise DomainError(
            "source_scope_forbidden",
            "Input admission belongs to its authenticated authority transaction",
        )


async def admit_text_in(
    self: MaterialService,
    tx: WriteTx,
    ctx: CallContext,
    document: ImportDocument,
    lease: ContentLease | None = None,
) -> InputAdmission:
    _require_transaction(tx, ctx)
    if document.material_id is not None:
        raise DomainError(
            "existing_input", "Existing resources require the original SourceRead context"
        )
    material = await self.put_in(tx, ctx, document, lease)
    event = await self.observe_in(tx, ctx, material["id"], document, kind="import")
    await _record_admission(tx, ctx, material["id"], event["id"], True)
    return InputAdmission(material, event, _context_for_input(ctx, material["id"], event["id"]))


async def admit_blob_in(
    self: MaterialService,
    tx: WriteTx,
    ctx: CallContext,
    lease: ContentLease,
    document: BlobDocument,
    info: MediaInfo | None = None,
) -> InputAdmission:
    _require_transaction(tx, ctx)
    material = await self.put_blob_in(tx, ctx, lease, document, info)
    event = await self.observe_in(tx, ctx, material["id"], document, kind="import")
    await _record_admission(tx, ctx, material["id"], event["id"], True)
    return InputAdmission(material, event, _context_for_input(ctx, material["id"], event["id"]))


async def _record_admission(
    tx: WriteTx,
    ctx: CallContext,
    material_id: str,
    event_id: str,
    new_material: bool,
    source_references: tuple[MediaReference, ...] = (),
) -> None:
    await tx.connection.execute(
        input_admissions.insert().values(
            observation_id=event_id,
            material_id=material_id,
            new_material=new_material,
            source_references=[
                reference.model_dump(mode="json") for reference in source_references
            ],
            namespace_id=ctx.namespace_id,
            subject_id=ctx.require_subject(),
            caller_id=ctx.caller_id,
            operation_key=tx.operation_key,
            commit_seq=tx.commit_seq,
        )
    )


async def admit_observation_in(
    self: MaterialService,
    tx: WriteTx,
    ctx: CallContext,
    material_id: str,
    document: ImportDocument | BlobDocument,
    *,
    sources: tuple[MediaReference, ...] = (),
) -> InputAdmission:
    _require_transaction(tx, ctx)
    selected = sources or (MediaReference(kind="material", id=material_id),)
    validated: list[MediaReference] = []
    material = None
    for reference in selected:
        value, _, event_id = await self.media_source_in(tx, ctx, reference)
        if value["id"] != material_id:
            raise DomainError(
                "source_material_conflict", "Observation source belongs to another Material"
            )
        material = value
        validated.append(reference.model_copy(update={"observation_id": event_id}))
    assert material is not None
    event = await self.observe_in(
        tx, ctx, material_id, document, kind="import", sources=tuple(validated)
    )
    await _record_admission(tx, ctx, material_id, event["id"], False, tuple(validated))
    regions = (
        tuple(reference.id for reference in validated if reference.kind == "source_region")
        if all(reference.kind == "source_region" for reference in validated)
        else ()
    )
    return InputAdmission(
        material,
        event,
        _context_for_input(ctx, material_id, event["id"], new_material=False, region_ids=regions),
    )


async def restore_input_in(
    self: MaterialService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    material_ids: tuple[str, ...] | None = None,
    *,
    operation_id: str,
    operation_kind: str,
    scope: str,
) -> RestoredInputAdmission:
    from pydantic import TypeAdapter

    from nous.platform.persistence.schema import operations

    from .service import MaterialRecord, SourceRecord, materials, observations

    key = operation_key(ctx, operation_kind, scope, operation_id)
    receipt_seq = await view.connection.scalar(
        select(operations.c.commit_seq).where(operations.c.operation_key == key)
    )
    if receipt_seq is None:
        raise DomainError(
            "input_admission_unavailable", "The original input operation is not retained"
        )
    statement = select(input_admissions).where(
        input_admissions.c.namespace_id == ctx.namespace_id,
        input_admissions.c.subject_id == ctx.require_subject(),
        input_admissions.c.caller_id == ctx.caller_id,
        input_admissions.c.operation_key == key,
    )
    if material_ids is not None:
        statement = statement.where(input_admissions.c.material_id.in_(material_ids))
    facts = (
        (
            await view.connection.execute(
                statement.order_by(
                    input_admissions.c.new_material.desc(), input_admissions.c.observation_id
                )
            )
        )
        .mappings()
        .all()
    )
    if material_ids is not None and not set(material_ids).issubset(
        {row["material_id"] for row in facts}
    ):
        raise DomainError(
            "input_admission_unavailable", "The receipt does not establish this submitted input"
        )
    effective = ctx
    fresh_materials: set[str] = set()
    for fact in facts:
        value = await view.connection.scalar(
            select(materials.c.record).where(
                materials.c.id == fact["material_id"], materials.c.namespace_id == ctx.namespace_id
            )
        )
        event_value = await view.connection.scalar(
            select(observations.c.record).where(
                observations.c.id == fact["observation_id"],
                observations.c.material_id == fact["material_id"],
                observations.c.namespace_id == ctx.namespace_id,
                observations.c.subject_id == ctx.require_subject(),
                observations.c.commit_seq == receipt_seq,
                observations.c.active.is_(True),
            )
        )
        if fact["commit_seq"] != receipt_seq or value is None or event_value is None:
            raise DomainError(
                "input_admission_unavailable",
                "The admission's original resource facts are unavailable",
            )
        material = TypeAdapter(MaterialRecord).validate_python(value)
        event = TypeAdapter(SourceRecord).validate_python(event_value)
        if (
            event["id"] != fact["observation_id"]
            or event["material_id"] != material["id"]
            or material["id"] != fact["material_id"]
            or material["namespace_id"] != ctx.namespace_id
            or event["namespace_id"] != ctx.namespace_id
            or event["subject_id"] != ctx.subject_id
        ):
            raise DomainError(
                "input_admission_unavailable",
                "The admitted identities differ from their authoritative resources",
            )
        if (
            material.get("purged")
            or event.get("purged")
            or (fact["new_material"] and material["commit_seq"] != receipt_seq)
        ):
            raise DomainError(
                "input_admission_unavailable", "The original submitted content is unavailable"
            )
        if fact["new_material"]:
            fresh_materials.add(material["id"])
        elif material["id"] not in fresh_materials:
            references = TypeAdapter(tuple[MediaReference, ...]).validate_python(
                fact["source_references"]
            )
            for reference in references:
                await self.media_source_in(view, ctx, reference)
        references = TypeAdapter(tuple[MediaReference, ...]).validate_python(
            fact["source_references"]
        )
        regions = (
            tuple(reference.id for reference in references if reference.kind == "source_region")
            if references and all(reference.kind == "source_region" for reference in references)
            else ()
        )
        effective = _context_for_input(
            effective,
            material["id"],
            event["id"],
            new_material=bool(fact["new_material"]),
            region_ids=regions,
        )
    return RestoredInputAdmission(
        effective,
        int(receipt_seq),
        tuple(dict.fromkeys(str(fact["material_id"]) for fact in facts)),
        tuple(str(fact["observation_id"]) for fact in facts),
    )
