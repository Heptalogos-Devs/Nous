"""A host's real description can precede the bytes it describes."""

from __future__ import annotations

import re
from hashlib import file_digest, sha256
from typing import TYPE_CHECKING
from uuid import uuid4

import sqlalchemy as sa
from nous_sdk.contracts import (
    AdoptCognition,
    CallContext,
    CognitiveRef,
    Contract,
    DomainError,
    EvidenceLink,
    ImportDocument,
    MediaReference,
    MemoryBody,
)
from nous_sdk.contracts.derivation import (
    CreateRepresentation,
    DerivationInput,
    DerivationProducer,
    TextRepresentation,
)
from nous_sdk.contracts.lifecycle import MemoryChange
from pydantic import Field, JsonValue, TypeAdapter

from nous.identity.access import require_namespace_in
from nous.platform.concurrency import owned_thread
from nous.platform.content import ContentObject
from nous.platform.persistence import ReadView, WriteTx, metadata
from nous.platform.persistence.authority import canonical_json

from .derived_producer import digest
from .service import SourceRecord, materials, require_observation_in

if TYPE_CHECKING:
    from .description_deliveries import DescriptionDeliveryService


descriptions = sa.Table(
    "material_external_descriptions",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("namespace_id", sa.String, nullable=False, index=True),
    
    sa.Column("material_id", sa.String, nullable=False),
    sa.Column("observation_id", sa.String, nullable=False),
    sa.Column("record", sa.JSON, nullable=False),
)


class DescriptionBinding(Contract):
    observation_id: str
    material_id: str
    reference: CognitiveRef


class ExternalDescription(Contract):
    id: str
    content_hash: str
    source: MediaReference
    source_kind: str
    confirmation: str
    details: dict[str, JsonValue] = Field(default_factory=dict)
    conversation_refs: tuple[str, ...] | None = None
    bindings: tuple[DescriptionBinding, ...] = ()


async def load_in(
    self: DescriptionDeliveryService, view: ReadView | WriteTx, ctx: CallContext, identifier: str
) -> ExternalDescription:
    raw = await view.connection.scalar(
        sa.select(descriptions.c.record).where(
            descriptions.c.id == identifier,
            descriptions.c.namespace_id == ctx.namespace_id,
            
        )
    )
    if raw is None:
        raise DomainError("description_not_found", "External description is unavailable")
    value = ExternalDescription.model_validate(raw)
    await self.cognition.materials.accepted_source_in(
        view,
        ctx,
        value.source.observation_id or "",
        value.source,
        derivations=self.cognition.derivations,
    )
    return value


async def record_external(
    self: DescriptionDeliveryService,
    ctx: CallContext,
    *,
    content_hash: str,
    text: str,
    source_kind: str,
    confirmation: str,
    details: dict[str, JsonValue],
    conversation_refs: tuple[str, ...] | None,
) -> ExternalDescription:
    if re.fullmatch(r"[0-9a-fA-F]{32}(?:[0-9a-fA-F]{32})?", content_hash) is None:
        raise DomainError("description_hash_required", "Supply the declared image content hash")
    payload = {
        "content_hash": content_hash.casefold(),
        "text": text,
        "source_kind": source_kind,
        "confirmation": confirmation,
        "details": details,
        "conversation_refs": conversation_refs,
    }
    if not text.strip():
        raise DomainError("description_required", "Supply the actual external description")
    operation = "external-description:" + sha256(canonical_json(payload).encode()).hexdigest()
    async with self.authority.write(
        ctx,
        operation,
        operation_kind="media.description.source",
        scope=ctx.namespace_id,
        payload=payload,
    ) as tx:
        await require_namespace_in(tx, ctx, "memory")
        if tx.replayed:
            previous = ExternalDescription.model_validate(tx.replay_result)
            return await load_in(self, tx, ctx, previous.id)
        report = await self.cognition.materials.admit_text_in(
            tx,
            ctx,
            ImportDocument(
                text=text,
                source_type="external_description",
                origin={"caller_id": ctx.caller_id, "source_kind": source_kind},
            ),
        )
        value = ExternalDescription(
            id=str(uuid4()),
            content_hash=content_hash.casefold(),
            source=MediaReference(
                kind="material", id=report.material["id"], observation_id=report.event["id"]
            ),
            source_kind=source_kind,
            confirmation=confirmation,
            details=details,
            conversation_refs=conversation_refs,
        )
        await tx.connection.execute(
            descriptions.insert().values(
                id=value.id,
                namespace_id=ctx.namespace_id,
                
                material_id=report.material["id"],
                observation_id=report.event["id"],
                record=value.model_dump(mode="json"),
            )
        )
        return tx.result(value)


async def matching_external(
    self: DescriptionDeliveryService, ctx: CallContext, observation_id: str
) -> tuple[ExternalDescription, ...]:
    async with self.authority.read(ctx) as view:
        event = TypeAdapter(SourceRecord).validate_python(
            (await require_observation_in(view, ctx, observation_id))["record"]
        )
        original = await self.cognition.materials.read_in(view, ctx, event["material_id"])
        raw_values = (
            await view.connection.execute(
                sa.select(descriptions.c.record).where(
                    descriptions.c.namespace_id == ctx.namespace_id,
                    
                )
            )
        ).scalars()
        values: list[ExternalDescription] = []
        for raw in raw_values:
            value = ExternalDescription.model_validate(raw)
            try:
                values.append(await load_in(self, view, ctx, value.id))
            except DomainError as error:
                if error.code != "source_unavailable":
                    raise
    hashes = {original["content_hash"]}
    if any(len(value.content_hash) == 32 for value in values):
        store = self.cognition.materials.content_store
        if store is None:
            raise RuntimeError("Media description binding requires original storage")
        async with store.protect(
            ContentObject(original["content_hash"], original["byte_len"], original["media_type"])
        ) as path:

            def legacy_hash() -> str:
                with path.open("rb") as stream:
                    return file_digest(stream, "md5").hexdigest()

            hashes.add(await owned_thread(legacy_hash))
    return tuple(
        value
        for value in values
        if value.content_hash in hashes
        and (
            value.conversation_refs is None
            or event["origin"].get("conversation_ref", "") in value.conversation_refs
        )
        and not any(binding.observation_id == observation_id for binding in value.bindings)
    )


async def pending_external_count(self: DescriptionDeliveryService, ctx: CallContext) -> int:
    async with self.authority.read(ctx) as view:
        rows = (
            await view.connection.execute(
                sa.select(descriptions.c.record).where(
                    descriptions.c.namespace_id == ctx.namespace_id,
                    
                )
            )
        ).scalars()
        count = 0
        for raw in rows:
            value = ExternalDescription.model_validate(raw)
            if value.bindings:
                continue
            try:
                await load_in(self, view, ctx, value.id)
            except DomainError as error:
                if error.code != "source_unavailable":
                    raise
            else:
                count += 1
        return count


async def adopt_external(
    self: DescriptionDeliveryService,
    ctx: CallContext,
    identifier: str,
    observation_id: str,
    *,
    supersedes: CognitiveRef | None = None,
) -> dict[str, JsonValue]:
    payload = {
        "description_id": identifier,
        "observation_id": observation_id,
        "supersedes": supersedes,
    }
    operation = "description-binding:" + sha256(canonical_json(payload).encode()).hexdigest()
    async with self.authority.write(
        ctx,
        operation,
        operation_kind="media.description.bind",
        scope=ctx.namespace_id,
        payload=payload,
    ) as tx:
        if tx.replayed:
            result = TypeAdapter(dict[str, JsonValue]).validate_python(tx.replay_result)
            await self.cognition.get_in(tx, ctx, CognitiveRef.model_validate(result["reference"]))
            return result
        description = await load_in(self, tx, ctx, identifier)
        event = TypeAdapter(SourceRecord).validate_python(
            (await require_observation_in(tx, ctx, observation_id))["record"]
        )
        if (
            event["kind"] != "contact"
            or description.conversation_refs is not None
            and event["origin"].get("conversation_ref", "") not in description.conversation_refs
        ):
            raise DomainError(
                "description_contact_scope",
                "Description requires an actual contact within its supplied scope",
            )
        candidates = await matching_external(self, ctx, observation_id)
        if not any(value.id == identifier for value in candidates):
            existing = next(
                (
                    binding
                    for binding in description.bindings
                    if binding.observation_id == observation_id
                ),
                None,
            )
            if existing is not None:
                await self.cognition.get_in(tx, ctx, existing.reference)
                return tx.result(
                    {
                        "success": True,
                        "reference": existing.reference.model_dump(mode="json"),
                        "observation_id": existing.reference.exact(),
                    }
                )
            raise DomainError(
                "description_content_mismatch",
                "Description hash does not match the encountered original",
            )
        captured = event.get("contact_source")
        accepted = await self.cognition.materials.accepted_source_in(
            tx,
            ctx,
            observation_id,
            MediaReference.model_validate(captured) if captured is not None else None,
            derivations=self.cognition.derivations,
        )
        report = await self.cognition.materials.read_in(tx, ctx, description.source.id)
        text = report["text"]
        if text is None:
            raise DomainError(
                "description_text_required",
                "External description requires its original text report",
            )
        config = {
            "implementation": "external_host_description",
            "source_kind": description.source_kind,
        }
        representation = await self.cognition.derivations.adopt_in(
            tx,
            ctx,
            CreateRepresentation(
                inputs=(
                    DerivationInput(ordinal=0, source=accepted.source),
                    DerivationInput(ordinal=1, source=description.source),
                ),
                kind="image_description",
                payload=TextRepresentation(text=text),
                producer=DerivationProducer(
                    implementation="external_host_description",
                    signature=digest((config, identifier, observation_id)),
                    config_digest=digest(config),
                ),
            ),
        )
        body = MemoryBody(
            text=text,
            media=(accepted.source, representation.reference.media()),
            role="declarative",
            contact_observation_id=observation_id,
            external_ids={
                "image_description": representation.reference.media().exact(),
                "external_description": identifier,
            },
            structured={
                "confirmation": description.confirmation,
                "source_kind": description.source_kind,
                "evidence": description.details,
            },
        )
        if supersedes is not None:
            previous = await self.cognition.get_in(tx, ctx, supersedes)
            if (
                not isinstance(previous.body, MemoryBody)
                or "image_description" not in previous.body.external_ids
                or observation_id not in previous.source_roots
            ):
                raise DomainError(
                    "image_observation_required",
                    "Superseded Memory must describe the same actual contact",
                )
            memory = await self.memories.change_memory_in(
                tx,
                ctx,
                MemoryChange(reference=supersedes, action="revise", body=body),
                revision_evidence=(EvidenceLink(observation_id=description.source.observation_id),),
            )
        else:
            memory = await self.cognition.adopt_in(
                tx,
                ctx,
                AdoptCognition(
                    body=body,
                    evidence=(EvidenceLink(observation_id=description.source.observation_id),),
                ),
            )
        binding = DescriptionBinding(
            observation_id=observation_id,
            material_id=event["material_id"],
            reference=memory.reference,
        )
        description = description.model_copy(update={"bindings": (*description.bindings, binding)})
        await tx.connection.execute(
            descriptions.update()
            .where(descriptions.c.id == identifier)
            .values(record=description.model_dump(mode="json"))
        )
        delivery_id = (
            await self.enqueue_in(tx, ctx, observation_id, memory.reference)
            if description.source_kind == "model_description"
            else None
        )
        return tx.result(
            {
                "success": True,
                "reference": memory.reference.model_dump(mode="json"),
                "observation_id": memory.reference.exact(),
                "delivery_id": delivery_id,
                "representation": representation.reference.model_dump(mode="json"),
                "asset_id": event["material_id"],
                "content_hash": accepted.material["content_hash"],
            }
        )


async def purge_external_in(
    self: DescriptionDeliveryService,
    tx: WriteTx,
    *,
    material_ids: tuple[str, ...],
    references: tuple[CognitiveRef, ...],
) -> None:
    selected = set(material_ids)
    if selected:
        hashes = set(
            (
                await tx.connection.execute(
                    sa.select(materials.c.content_hash).where(materials.c.id.in_(selected))
                )
            ).scalars()
        )
        rows = (
            await tx.connection.execute(
                sa.select(descriptions).where(descriptions.c.namespace_id == tx.ctx.namespace_id)
            )
        ).mappings()
        for row in rows:
            value = ExternalDescription.model_validate(row["record"])
            if (
                row["material_id"] in selected
                or value.content_hash in hashes
                or any(binding.material_id in selected for binding in value.bindings)
            ):
                await tx.connection.execute(
                    descriptions.delete().where(descriptions.c.id == row["id"])
                )
