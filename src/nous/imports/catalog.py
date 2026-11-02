from __future__ import annotations

import asyncio
from dataclasses import dataclass
from hashlib import sha256
from typing import TYPE_CHECKING, Literal
from uuid import uuid4

from langchain_core.documents import Document
from nous_sdk.contracts import (
    CallContext,
    CatalogView,
    DomainError,
    FreezeCatalog,
    ImportDocument,
    ImportRecordError,
    ImportUnit,
    MediaReference,
    SourceRegion,
)
from nous_sdk.contracts.imports import TextImportPreview, TextPreviewUnit, TextSplitOptions
from pydantic import TypeAdapter
from sqlalchemy import select

from nous.identity.access import require_namespace_in
from nous.material.service import MaterialRecord, require_observation_in
from nous.platform.concurrency import owned_thread
from nous.platform.content import ContentLease, ContentObject
from nous.platform.persistence import ReadView, WriteTx

from .parsing import ParsedRecord, parse_document
from .schema import catalogs, inputs, units

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


@dataclass(frozen=True)
class FrozenCatalog:
    catalog: CatalogView
    context: CallContext


async def prepare_document(
    self: ImportService, ctx: CallContext, document: ImportDocument
) -> PreparedInput:
    material: MaterialRecord | None = None
    if document.material_id is not None:
        material = await self.materials.read(ctx, document.material_id)
        if material["media_type"].startswith(("image/", "audio/", "video/")):
            return PreparedInput(
                document,
                "",
                material,
                [
                    ParsedRecord(
                        index=0,
                        record=None,
                        media_required=True,
                    )
                ],
            )
        if document.format == "pdf" or material["media_type"] == "application/pdf":
            from .document_extraction import pdf_records

            if self.content_store is None:
                raise DomainError("content_unavailable", "Original PDF bytes are unavailable")
            async with self.content_store.protect(
                ContentObject(
                    material["content_hash"], material["byte_len"], material["media_type"]
                )
            ) as path:
                parsed = await asyncio.to_thread(pdf_records, path)
            return PreparedInput(document, "", material, parsed)
        if document.format in {"parquet", "sqlite", "graphml"}:
            from .tabular import graphml_records, parquet_records, sqlite_records

            if self.content_store is None:
                raise DomainError(
                    "content_unavailable", "Structured original bytes are unavailable"
                )
            reader = {
                "parquet": parquet_records,
                "sqlite": sqlite_records,
                "graphml": graphml_records,
            }[document.format]
            async with self.content_store.protect(
                ContentObject(
                    material["content_hash"], material["byte_len"], material["media_type"]
                )
            ) as path:
                parsed = await owned_thread(lambda: reader(path, document))
            return PreparedInput(document, "", material, parsed)
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
    from .document_extraction import html_records

    if document.format == "media":
        raise DomainError(
            "media_original_required",
            "Media import requires an uploaded image, audio or video original",
        )
    if document.format == "pdf":
        raise DomainError(
            "pdf_original_required", "PDF import requires its uploaded original material_id"
        )
    if document.format in {"parquet", "sqlite", "graphml"}:
        raise DomainError(
            "structured_original_required",
            "Structured import requires its uploaded original material_id",
        )
    if (
        document.format == "html"
        or document.media_type == "text/html"
        or (material is not None and material["media_type"] == "text/html")
    ):
        records = await asyncio.to_thread(html_records, content)
    else:
        try:
            records = parse_document(
                content,
                TypeAdapter(Literal["text", "json", "jsonl"]).validate_python(
                    "text" if document.format == "markdown" else document.format
                ),
                identifier_scope="dataset:"
                + (
                    document.origin.get("dataset_ref")
                    or document.file_identity
                    or document.source
                    or document.filename
                ),
            )
        except DomainError as error:
            if error.code not in {"empty_import", "invalid_import_json"}:
                raise
            records = [ParsedRecord(index=0, record=None, error=str(error))]
    if document.selection is not None:
        from .tabular import selected

        filtered: list[ParsedRecord] = []
        for parsed in records:
            if parsed.record is None or selected(parsed.record, parsed.index + 1, document):
                filtered.append(parsed)
            if document.selection.limit is not None and len(filtered) >= document.selection.limit:
                break
        records = filtered
    return PreparedInput(document, content, material, records)


async def freeze_catalog(
    self: ImportService, ctx: CallContext, command: FreezeCatalog
) -> CatalogView:
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
    from .document_extraction import chunks_for

    split = [[chunks_for(parsed, command) for parsed in item.records] for item in prepared]
    async with self.authority.write(ctx) as tx:
        frozen = await self.freeze_catalog_in(tx, ctx, command, leases, prepared, split)
        return frozen.catalog


async def freeze_catalog_in(
    self: ImportService,
    tx: WriteTx,
    ctx: CallContext,
    command: FreezeCatalog,
    leases: list[ContentLease | None],
    prepared: list[PreparedInput],
    split: list[list[list[Document]]],
    *,
    admitted_observations: dict[str, str] | None = None,
) -> FrozenCatalog:
    await require_namespace_in(tx, ctx, "memory")
    from nous_sdk.contracts.namespace_policy import FormationPolicy

    from .document_extraction import chunks_for

    policy = await self.configuration.policy(ctx, "formation", FormationPolicy)
    chunk_size = min(command.chunk_size, policy.max_chunk_chars)
    overlap = (
        command.chunk_overlap
        if "chunk_overlap" in command.model_fields_set
        else policy.chunk_overlap_chars
    )
    adjusted = command.model_copy(
        update={"chunk_size": chunk_size, "chunk_overlap": min(overlap, chunk_size - 1)}
    )
    if adjusted != command:
        command = adjusted
        split = [[chunks_for(parsed, command) for parsed in item.records] for item in prepared]
    catalog_id = str(uuid4())
    effective = ctx
    source_ids: list[str] = []
    frozen: list[ImportUnit] = []
    errors: list[ImportRecordError] = []
    for item, record_chunks, lease in zip(prepared, split, leases, strict=True):
        document = item.document
        initial_event = None
        if item.material:
            material = await self.materials.read_in(tx, ctx, item.material["id"])
            if admitted_observations and material["id"] in admitted_observations:
                initial_event = (
                    await require_observation_in(tx, ctx, admitted_observations[material["id"]])
                )["record"]
        else:
            admission = await self.materials.admit_text_in(tx, effective, document, lease)
            material, effective = admission.material, admission.context
            initial_event = admission.event
        source_id = str(uuid4())
        source_ids.append(source_id)
        source = {
            "id": source_id,
            "material_id": material["id"],
            "document": {
                **document.model_dump(mode="json", exclude={"text"}, exclude_unset=True),
                "material_id": material["id"],
            },
            "content_hash": material["content_hash"],
            "original_unit_count": sum(len(chunks) for chunks in record_chunks),
        }
        await tx.connection.execute(
            inputs.insert().values(
                id=source_id, catalog_id=catalog_id, material_id=material["id"], record=source
            )
        )
        for parsed, chunks in zip(item.records, record_chunks, strict=True):
            record = parsed.record
            if record is None and not parsed.media_required:
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
            if record is not None and record.source is None and document.source is not None:
                record = record.model_copy(update={"source": document.source})
            context = (
                record.model_dump(mode="json", exclude={"text"}, exclude_unset=True)
                if record
                else {}
            )
            if parsed.media_required:
                context["media_strategy"] = document.media_strategy
            actual = document.model_copy(
                update={
                    "event_time": (record.event_time if record else None) or document.event_time,
                    "observed_at": (record.observed_at if record else None) or document.observed_at,
                    "origin": {**document.origin, **(record.origin if record else {})},
                    "source_type": (record.source_type if record else None) or document.source_type,
                    "source": (record.source if record else None) or document.source,
                }
            )
            if record is not None and "validity" in record.model_fields_set:
                actual = actual.model_copy(update={"validity": record.validity})
            if not chunks:
                continue
            if (
                initial_event is not None
                and len(item.records) == 1
                and document.format in {"text", "markdown", "html"}
            ):
                event = initial_event
            else:
                admission = await self.materials.admit_observation_in(
                    tx, effective, material["id"], actual
                )
                event, effective = admission.event, admission.context
            for chunk in chunks:
                start = int(chunk.metadata["start_index"])
                region = (
                    SourceRegion(
                        id=str(uuid4()),
                        material_id=material["id"],
                        text_start=None
                        if parsed.pointer is not None or parsed.line_number is not None
                        else 0
                        if parsed.extraction and parsed.page_number is None
                        else start
                        if not parsed.page_number
                        else None,
                        text_end=None
                        if parsed.pointer is not None or parsed.line_number is not None
                        else len(item.content)
                        if parsed.extraction and parsed.page_number is None
                        else start + len(chunk.page_content)
                        if not parsed.page_number
                        else None,
                        page_start=parsed.page_number,
                        page_end=parsed.page_number,
                        json_pointer=parsed.pointer,
                        line_number=parsed.line_number,
                    )
                    if (not parsed.media_required or parsed.page_number is not None)
                    and not (parsed.extraction and parsed.page_number is None and not item.content)
                    else None
                )
                if region is not None:
                    await self.materials.register_region_in(tx, effective, region)
                selected_source = MediaReference(
                    kind="source_region" if region is not None else "material",
                    id=region.id if region is not None else material["id"],
                    observation_id=event["id"],
                )
                if parsed.extraction and record is not None:
                    from .extracted_source import adopt_extracted_in

                    selected_source = await adopt_extracted_in(
                        self,
                        tx,
                        effective,
                        selected_source,
                        record.text,
                        start,
                        start + len(chunk.page_content),
                        parsed.extraction,
                    )
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
                    source_region_id=region.id if region is not None else None,
                    source=selected_source,
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
                stored = unit.model_dump(mode="json", exclude={"text"})
                basic_context = {
                    "external_id",
                    "source",
                    "source_type",
                    "origin",
                    "event_time",
                    "observed_at",
                    "validity",
                    "media_strategy",
                }
                if document.format in {"json", "jsonl", "parquet", "sqlite", "graphml"} or set(
                    context
                ).difference(basic_context):
                    from nous_sdk.contracts.derivation import (
                        CreateRepresentation,
                        DerivationInput,
                        StructuredRepresentation,
                    )

                    parsed_value = await self.cognition.derivations.adopt_in(
                        tx,
                        effective,
                        CreateRepresentation(
                            inputs=(DerivationInput(ordinal=0, source=selected_source),),
                            kind="parsed_record",
                            strategy="preprocess",
                            processor="nous.import-parser/1",
                            payload=StructuredRepresentation(
                                schema_id="nous.import-unit/1",
                                value={
                                    "text": unit.text,
                                    "context": unit.context,
                                    "split_context": unit.split_context,
                                },
                            ),
                        ),
                    )
                    unit = unit.model_copy(update={"source": parsed_value.reference.media()})
                    stored.update(
                        source=unit.source.model_dump(mode="json"), context={}, split_context={}
                    )
                frozen.append(unit)
                await tx.connection.execute(
                    units.insert().values(id=unit.id, catalog_id=catalog_id, record=stored)
                )
    result = CatalogView(
        split_options=TextSplitOptions.model_validate(
            command.model_dump(include=set(TextSplitOptions.model_fields))
        ),
        id=catalog_id,
        namespace_id=ctx.namespace_id,
        units=tuple(frozen),
        input_source_ids=tuple(source_ids),
        original_unit_count=len(frozen),
        original_record_count=sum(len(item.records) for item in prepared),
        errors=tuple(errors),
        formation_catalog=await self.formation.formation_catalog_in(
            tx, effective, "\n".join(unit.text for unit in frozen)
        ),
    )
    await tx.connection.execute(
        catalogs.insert().values(
            id=catalog_id,
            namespace_id=ctx.namespace_id,
            record={
                **result.model_dump(mode="json", exclude={"units", "formation_catalog"}),
                "unit_ids": [unit.id for unit in result.units],
                "formation_references": [
                    entry.reference.model_dump(mode="json") for entry in result.formation_catalog
                ],
            },
        )
    )
    return FrozenCatalog(result, effective)


async def read_catalog_in(
    self: ImportService, view: ReadView | WriteTx, ctx: CallContext, catalog_id: str
) -> CatalogView:
    value = (
        await view.connection.execute(
            select(catalogs.c.record).where(
                catalogs.c.id == catalog_id,
                catalogs.c.namespace_id == ctx.namespace_id,
            )
        )
    ).scalar_one_or_none()
    if value is None:
        raise DomainError("catalog_not_found", "Import catalog is unavailable")
    from nous_sdk.contracts import CognitiveRef

    header = dict(value)
    unit_ids = header.pop("unit_ids")
    references = tuple(
        CognitiveRef.model_validate(item) for item in header.pop("formation_references", ())
    )
    hydrated = tuple(
        [await self.read_unit_in(view, ctx, unit_id, catalog_id=catalog_id) for unit_id in unit_ids]
    )
    hints = await self.formation.formation_catalog_in(view, ctx, references=references)
    return CatalogView.model_validate({**header, "units": hydrated, "formation_catalog": hints})


async def read_catalog(self: ImportService, ctx: CallContext, catalog_id: str) -> CatalogView:
    async with self.authority.read(ctx) as view:
        await require_namespace_in(view, ctx)
        return await self.read_catalog_in(view, ctx, catalog_id)


async def preview_text_import(
    self: ImportService,
    ctx: CallContext,
    document: ImportDocument,
    overrides: TextSplitOptions | None = None,
) -> TextImportPreview:
    from .document_extraction import chunks_for

    options = overrides or TextSplitOptions()
    item = await self.prepare_document(ctx, document)
    return TextImportPreview(
        content_hash=item.content_hash,
        byte_len=item.material["byte_len"] if item.material else len(item.content.encode()),
        options=options,
        units=tuple(
            TextPreviewUnit(
                text=chunk.page_content,
                start=int(chunk.metadata["start_index"]),
                end=int(chunk.metadata["start_index"]) + len(chunk.page_content),
                strategy=str(chunk.metadata.get("strategy", "narrative")),
                requires_llm=bool(chunk.metadata.get("requires_llm", True)),
                context={
                    **chunk.metadata.get("split_context", {}),
                    "record_index": parsed.index,
                    "page_number": parsed.page_number,
                    "extraction": parsed.extraction,
                },
            )
            for parsed in item.records
            for chunk in chunks_for(parsed, options)
        ),
        errors=tuple(
            f"Record {parsed.index + 1}: {parsed.error}" for parsed in item.records if parsed.error
        ),
    )
