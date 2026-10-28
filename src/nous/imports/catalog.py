from __future__ import annotations

import asyncio
from dataclasses import dataclass
from hashlib import sha256
from typing import TYPE_CHECKING
from uuid import uuid4

from langchain_core.documents import Document
from nous_wave.contracts import (
    CallContext,
    CatalogView,
    DomainError,
    FreezeCatalog,
    ImportDocument,
    ImportRecordError,
    ImportUnit,
    SourceRegion,
)
from sqlalchemy import select

from nous.identity.service import require_subject_in
from nous.material.service import MaterialRecord
from nous.platform.content import ContentLease, ContentObject
from nous.platform.persistence import ReadView, WriteTx

from .parsing import ParsedRecord, parse_document
from .schema import catalogs, inputs, units
from .splitting import split_text

if TYPE_CHECKING:
    from .service import ImportService


@dataclass(frozen=True)
class PreparedInput:
    document: ImportDocument
    content: str
    material: MaterialRecord | None
    records: list[ParsedRecord]

    @property
    def content_hash(self) -> str:
        return (
            self.material["content_hash"]
            if self.material is not None
            else sha256(self.content.encode()).hexdigest()
        )


async def prepare_document(
    self: ImportService, ctx: CallContext, document: ImportDocument
) -> PreparedInput:
    material: MaterialRecord | None = None
    if document.material_id is not None:
        material = await self.materials.read(ctx, document.material_id)
        content = material["text"]
        if content is None:
            if self.content_store is None:
                raise DomainError("content_unavailable", "Original bytes are unavailable")
            async with self.content_store.protect(
                ContentObject(
                    material["content_hash"], material["byte_len"], material["media_type"]
                )
            ) as path:
                content = (await asyncio.to_thread(path.read_bytes)).decode("utf-8-sig")
    else:
        content = document.inline_text()
    return PreparedInput(document, content, material, parse_document(content, document.format))


async def freeze_catalog(
    self: ImportService, ctx: CallContext, command: FreezeCatalog
) -> CatalogView:
    receipt = await self.authority.receipt(
        ctx,
        command.operation_id,
        operation_kind="imports.freeze",
        scope=ctx.require_subject(),
        payload=command,
    )
    if receipt is not None:
        return CatalogView.model_validate(receipt.result)
    if command.chunk_overlap >= command.chunk_size:
        raise DomainError("invalid_split", "Chunk overlap must be smaller than chunk size")
    prepared = [await self.prepare_document(ctx, document) for document in command.inputs]
    leases: list[ContentLease | None] = []
    try:
        for item in prepared:
            leases.append(
                await self.content_store.publish_bytes(
                    item.content.encode(), item.document.media_type
                )
                if self.content_store and item.material is None
                else None
            )
        return await self.freeze_catalog_prepared(ctx, command, leases, prepared)
    finally:
        if self.content_store:
            for lease in leases:
                if lease is not None:
                    await self.content_store.release(lease)


async def freeze_catalog_prepared(
    self: ImportService,
    ctx: CallContext,
    command: FreezeCatalog,
    leases: list[ContentLease | None],
    prepared: list[PreparedInput],
) -> CatalogView:
    split = [
        [
            split_text(parsed.record.text, command) if parsed.record else []
            for parsed in item.records
        ]
        for item in prepared
    ]
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="imports.freeze",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            return CatalogView.model_validate(tx.replay_result)
        return tx.result(await self.freeze_catalog_in(tx, ctx, command, leases, prepared, split))


async def freeze_catalog_in(
    self: ImportService,
    tx: WriteTx,
    ctx: CallContext,
    command: FreezeCatalog,
    leases: list[ContentLease | None],
    prepared: list[PreparedInput],
    split: list[list[list[Document]]],
) -> CatalogView:
    await require_subject_in(tx, ctx, "memory")
    catalog_id = str(uuid4())
    source_ids: list[str] = []
    frozen: list[ImportUnit] = []
    errors: list[ImportRecordError] = []
    for item, record_chunks, lease in zip(prepared, split, leases, strict=True):
        document = item.document
        material = (
            await self.materials.read_in(tx, ctx, item.material["id"])
            if item.material
            else await self.materials.put_in(tx, ctx, document, lease)
        )
        source_id = str(uuid4())
        source_ids.append(source_id)
        source = {
            "id": source_id,
            "material_id": material["id"],
            "document": document.model_dump(mode="json"),
            "content_hash": material["content_hash"],
        }
        await tx.connection.execute(
            inputs.insert().values(
                id=source_id, catalog_id=catalog_id, material_id=material["id"], record=source
            )
        )
        for parsed, chunks in zip(item.records, record_chunks, strict=True):
            record = parsed.record
            if record is None:
                errors.append(
                    ImportRecordError(
                        input_source_id=source_id,
                        record_index=parsed.index,
                        line_number=parsed.line_number,
                        external_id=parsed.external_id,
                        code="invalid_import_record",
                        message=parsed.error or "Record could not be decoded",
                    )
                )
                continue
            context = record.model_dump(mode="json", exclude={"text"})
            actual = document.model_copy(
                update={
                    "event_time": record.event_time or document.event_time,
                    "observed_at": record.observed_at or document.observed_at,
                    "origin": {**document.origin, **record.origin},
                    "source_type": record.source_type or document.source_type,
                }
            )
            for chunk in chunks:
                start = int(chunk.metadata["start_index"])
                event = await self.materials.observe_in(tx, ctx, material["id"], actual)
                region = SourceRegion(
                    id=str(uuid4()),
                    material_id=material["id"],
                    text_start=start,
                    text_end=start + len(chunk.page_content),
                    json_pointer=parsed.pointer,
                    line_number=parsed.line_number,
                )
                await self.materials.register_region_in(tx, ctx, region)
                unit = ImportUnit(
                    id=str(uuid4()),
                    catalog_id=catalog_id,
                    input_source_id=source_id,
                    material_id=material["id"],
                    ordinal=len(frozen),
                    text=chunk.page_content,
                    start=start,
                    end=start + len(chunk.page_content),
                    record_index=parsed.index,
                    observation_id=event["id"],
                    source_region_id=region.id,
                    json_pointer=parsed.pointer,
                    line_number=parsed.line_number,
                    context=context,
                    strategy=str(chunk.metadata.get("strategy", "narrative")),
                    requires_llm=bool(chunk.metadata.get("requires_llm", True)),
                    split_context=chunk.metadata.get("split_context", {}),
                    formation_options={
                        "extract_time": command.extract_time,
                        "timezone": command.timezone,
                        "reference_time": command.reference_time.model_dump(mode="json")
                        if command.reference_time
                        else None,
                        "chat_reference_time": command.chat_reference_time,
                        "content_category": command.content_category,
                    },
                )
                frozen.append(unit)
                await tx.connection.execute(
                    units.insert().values(
                        id=unit.id, catalog_id=catalog_id, record=unit.model_dump(mode="json")
                    )
                )
    result = CatalogView(
        id=catalog_id,
        namespace_id=ctx.namespace_id,
        subject_id=ctx.require_subject(),
        units=tuple(frozen),
        input_source_ids=tuple(source_ids),
        original_unit_count=len(frozen),
        original_record_count=sum(len(item.records) for item in prepared),
        errors=tuple(errors),
        formation_catalog=await self.cognition.formation_catalog_in(
            tx, ctx, "\n".join(unit.text for unit in frozen)
        ),
    )
    await tx.connection.execute(
        catalogs.insert().values(
            id=catalog_id,
            namespace_id=ctx.namespace_id,
            subject_id=ctx.require_subject(),
            record=result.model_dump(mode="json"),
        )
    )
    return result


async def read_catalog_in(
    self: ImportService, view: ReadView | WriteTx, ctx: CallContext, catalog_id: str
) -> CatalogView:
    value = (
        await view.connection.execute(
            select(catalogs.c.record).where(
                catalogs.c.id == catalog_id,
                catalogs.c.namespace_id == ctx.namespace_id,
                catalogs.c.subject_id == ctx.subject_id,
            )
        )
    ).scalar_one_or_none()
    if value is None:
        raise DomainError("catalog_not_found", "Import catalog is unavailable")
    result = CatalogView.model_validate(value)
    if ctx.allowed_source_ids is not None and any(
        unit.material_id not in ctx.allowed_source_ids for unit in result.units
    ):
        raise DomainError(
            "source_unavailable", "Catalog sources are outside the current read scope"
        )
    return result


async def read_catalog(self: ImportService, ctx: CallContext, catalog_id: str) -> CatalogView:
    async with self.authority.read(ctx) as view:
        await require_subject_in(view, ctx)
        return await self.read_catalog_in(view, ctx, catalog_id)
