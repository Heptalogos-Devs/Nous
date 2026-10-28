"""Frozen import units, atomic adoption, and durable file completion facts."""

from __future__ import annotations

import json
import asyncio
from dataclasses import dataclass
import math
from hashlib import sha256
from uuid import uuid4

from langchain_text_splitters import RecursiveCharacterTextSplitter
from nous_wave.contracts import (
    AdmitFiles,
    AdoptCognition,
    CallContext,
    CatalogView,
    CognitiveRef,
    CognitiveTimes,
    DomainError,
    EvidenceLink,
    FileSuccess,
    FormationProposal,
    FreezeCatalog,
    ImportDocument,
    ImportRecord, ImportRecordError, SourceRegion,
    ImportResult,
    ImportUnit,
    MemoryBody,
    ProcessCatalog,
    UnitOutput,
)
from pydantic import TypeAdapter
from sqlalchemy import JSON, Column, String, Table, delete, select

from nous.cognition.models import ModelServices
from nous.cognition.service import CognitionService, body_text, vectors
from nous.identity.service import require_subject_in
from nous.material.service import MaterialService, MaterialRecord, require_observation_in
from nous.platform.content import ContentLease, ContentStore, ContentObject
from nous.platform.execution import JobSpec
from nous.platform.persistence import Authority, ReadView, WriteTx, metadata

from .parsing import ParsedRecord, parse_document

@dataclass(frozen=True)
class PreparedInput:
    document: ImportDocument
    content: str
    material: MaterialRecord | None
    records: list[ParsedRecord]

catalogs = Table(
    "import_catalogs",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False),
    Column("subject_id", String, nullable=False),
    Column("record", JSON, nullable=False),
)
inputs = Table(
    "import_inputs",
    metadata,
    Column("id", String, primary_key=True),
    Column("catalog_id", String, nullable=False, index=True),
    Column("material_id", String, nullable=False),
    Column("record", JSON, nullable=False),
)
units = Table(
    "import_units",
    metadata,
    Column("id", String, primary_key=True),
    Column("catalog_id", String, nullable=False, index=True),
    Column("record", JSON, nullable=False),
)
outputs = Table(
    "import_outputs",
    metadata,
    Column("id", String, primary_key=True),
    Column("unit_id", String, nullable=False, index=True),
    Column("operation_id", String, nullable=False),
    Column("record", JSON, nullable=False),
)
successes = Table(
    "import_file_successes",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False),
    Column("subject_id", String, nullable=False),
    Column("input_source_id", String, nullable=False),
    Column("record", JSON, nullable=False),
)
manifest = Table(
    "import_manifest_heads",
    metadata,
    Column("key", String, primary_key=True),
    Column("success_id", String, nullable=False),
)
requests = Table(
    "import_requests",
    metadata,
    Column("id", String, primary_key=True),
    Column("record", JSON, nullable=False),
)


def manifest_key(ctx: CallContext, document: ImportDocument, policy: str, content_hash: str | None = None) -> str:
    digest = content_hash or sha256(document.inline_text().encode()).hexdigest()
    key = document.file_identity if policy == "manifest" and document.file_identity else digest
    return sha256(
        json.dumps(
            [ctx.namespace_id, ctx.subject_id, document.origin, policy, key], sort_keys=True
        ).encode()
    ).hexdigest()


class ImportService:
    def __init__(
        self,
        authority: Authority,
        cognition: CognitionService,
        models: ModelServices,
        content_store: ContentStore | None = None,
        materials: MaterialService | None = None,
    ) -> None:
        self.authority = authority
        self.cognition = cognition
        self.models = models
        self.materials = materials or MaterialService(authority, content_store)
        self.content_store = self.materials.content_store

    async def _prepare_document(self, ctx: CallContext, document: ImportDocument) -> PreparedInput:
        material: MaterialRecord | None = None
        if document.material_id is not None:
            material = await self.materials.read(ctx, document.material_id)
            content = material["text"]
            if content is None:
                if self.content_store is None:
                    raise DomainError("content_unavailable", "Original bytes are unavailable")
                async with self.content_store.protect(ContentObject(material["content_hash"], material["byte_len"], material["media_type"])) as path:
                    content = (await asyncio.to_thread(path.read_bytes)).decode("utf-8-sig")
        else:
            content = document.inline_text()
        return PreparedInput(document, content, material, parse_document(content, document.format))

    async def freeze_catalog(self, ctx: CallContext, command: FreezeCatalog) -> CatalogView:
        receipt = await self.authority.receipt(ctx, command.operation_id, operation_kind="imports.freeze", scope=ctx.require_subject(), payload=command)
        if receipt is not None:
            return CatalogView.model_validate(receipt.result)
        if command.chunk_overlap >= command.chunk_size:
            raise DomainError("invalid_split", "Chunk overlap must be smaller than chunk size")
        prepared = [await self._prepare_document(ctx, document) for document in command.inputs]
        leases: list[ContentLease | None] = []
        try:
            for item in prepared:
                leases.append(await self.content_store.publish_bytes(item.content.encode(), item.document.media_type) if self.content_store and item.material is None else None)
            return await self._freeze_catalog_prepared(ctx, command, leases, prepared)
        finally:
            if self.content_store:
                for lease in leases:
                    if lease is not None:
                        await self.content_store.release(lease)

    async def _freeze_catalog_prepared(self, ctx: CallContext, command: FreezeCatalog, leases: list[ContentLease | None], prepared: list[PreparedInput]) -> CatalogView:
        splitter = RecursiveCharacterTextSplitter(chunk_size=command.chunk_size, chunk_overlap=command.chunk_overlap, add_start_index=True)
        # Decoding and splitting finish before the writer lock is taken.
        split = [[splitter.create_documents([parsed.record.text]) if parsed.record else [] for parsed in item.records] for item in prepared]
        async with self.authority.write(ctx, command.operation_id, operation_kind="imports.freeze", scope=ctx.require_subject(), payload=command) as tx:
            if tx.replayed:
                return CatalogView.model_validate(tx.replay_result)
            await require_subject_in(tx, ctx, "memory")
            catalog_id = str(uuid4())
            source_ids: list[str] = []
            frozen: list[ImportUnit] = []
            errors: list[ImportRecordError] = []
            for item, record_chunks, lease in zip(prepared, split, leases, strict=True):
                document = item.document
                material = await self.materials.read_in(tx, ctx, item.material["id"]) if item.material else await self.materials.put_in(tx, ctx, document, lease)
                source_id = str(uuid4())
                source_ids.append(source_id)
                source = {"id": source_id, "material_id": material["id"], "document": document.model_dump(mode="json"), "content_hash": material["content_hash"]}
                await tx.connection.execute(inputs.insert().values(id=source_id, catalog_id=catalog_id, material_id=material["id"], record=source))
                for parsed, chunks in zip(item.records, record_chunks, strict=True):
                    record = parsed.record
                    if record is None:
                        errors.append(ImportRecordError(input_source_id=source_id, record_index=parsed.index, line_number=parsed.line_number, external_id=parsed.external_id, code="invalid_import_record", message=parsed.error or "Record could not be decoded"))
                        continue
                    context = record.model_dump(mode="json", exclude={"text"})
                    actual = document.model_copy(update={"event_time": record.event_time or document.event_time, "observed_at": record.observed_at or document.observed_at, "origin": {**document.origin, **record.origin}, "source_type": record.source_type or document.source_type})
                    for chunk in chunks:
                        start = int(chunk.metadata["start_index"])
                        event = await self.materials.observe_in(tx, ctx, material["id"], actual)
                        region = SourceRegion(id=str(uuid4()), material_id=material["id"], text_start=start, text_end=start + len(chunk.page_content), json_pointer=parsed.pointer, line_number=parsed.line_number)
                        await self.materials.register_region_in(tx, ctx, region)
                        unit = ImportUnit(id=str(uuid4()), catalog_id=catalog_id, input_source_id=source_id, material_id=material["id"], ordinal=len(frozen), text=chunk.page_content, start=start, end=start + len(chunk.page_content), record_index=parsed.index, observation_id=event["id"], source_region_id=region.id, json_pointer=parsed.pointer, line_number=parsed.line_number, context=context)
                        frozen.append(unit)
                        await tx.connection.execute(units.insert().values(id=unit.id, catalog_id=catalog_id, record=unit.model_dump(mode="json")))
            result = CatalogView(id=catalog_id, namespace_id=ctx.namespace_id, subject_id=ctx.require_subject(), units=tuple(frozen), input_source_ids=tuple(source_ids), original_unit_count=len(frozen), original_record_count=sum(len(item.records) for item in prepared), errors=tuple(errors))
            await tx.connection.execute(catalogs.insert().values(id=catalog_id, namespace_id=ctx.namespace_id, subject_id=ctx.require_subject(), record=result.model_dump(mode="json")))
            return tx.result(result)

    async def read_catalog_in(
        self, view: ReadView | WriteTx, ctx: CallContext, catalog_id: str
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

    async def read_catalog(self, ctx: CallContext, catalog_id: str) -> CatalogView:
        async with self.authority.read(ctx) as view:
            await require_subject_in(view, ctx)
            return await self.read_catalog_in(view, ctx, catalog_id)

    async def submit_process(self, ctx: CallContext, command: ProcessCatalog):
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="imports.submit",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                return tx.replay_result
            await self.read_catalog_in(tx, ctx, command.catalog_id)
            request_id = str(uuid4())
            await tx.connection.execute(
                requests.insert().values(
                    id=request_id,
                    record={
                        "context": ctx.model_dump(mode="json"),
                        "command": command.model_dump(mode="json"),
                    },
                )
            )
            job = await tx.enqueue(
                JobSpec(
                    workflow_name="nous.imports.process", queue_name="formation", args=(request_id,)
                )
            )
            return tx.result(
                {
                    "request_id": request_id,
                    "job_id": job.workflow_id,
                    "catalog_id": command.catalog_id,
                }
            )

    async def run_process(self, request_id: str):
        async with self.authority.read() as view:
            row = (
                await view.connection.execute(
                    select(requests.c.record).where(requests.c.id == request_id)
                )
            ).scalar_one()
        result = await self.process_catalog(
                CallContext.model_validate(row["context"]),
                ProcessCatalog.model_validate(row["command"]),
                attempt_job_id=request_id,
            )
        return {"catalog_id": result.catalog.id, "output_ids": [output.id for output in result.outputs], "file_success_ids": [fact.id for fact in result.file_successes]}

    async def process_catalog(
        self, ctx: CallContext, command: ProcessCatalog, *, attempt_job_id: str | None = None
    ) -> ImportResult:
        catalog = await self.read_catalog(ctx, command.catalog_id)
        selected = (
            set(command.unit_ids)
            if command.unit_ids is not None
            else {unit.id for unit in catalog.units}
        )
        if not selected.issubset({unit.id for unit in catalog.units}):
            raise DomainError("unit_not_found", "Requested unit does not belong to this catalog")
        adopted: list[UnitOutput] = []
        for unit in catalog.units:
            if unit.id not in selected:
                continue
            if unit.purged:
                raise DomainError("source_purged", "A purged import unit cannot be processed")
            async with self.authority.read(ctx) as view:
                source = (
                    await view.connection.execute(
                        select(inputs.c.record).where(inputs.c.id == unit.input_source_id)
                    )
                ).scalar_one()
                existing = (
                    await view.connection.execute(
                        select(outputs.c.record).where(
                            outputs.c.unit_id == unit.id,
                            outputs.c.operation_id == command.operation_id,
                        )
                    )
                ).scalar_one_or_none()
            if existing:
                output = UnitOutput.model_validate(existing)
            else:
                model_ctx = ctx.model_copy(
                    update={"request_id": f"{command.operation_id}:{unit.id}:form"}
                )
                proposal = (
                    (await self.models.form(model_ctx, unit.text, source))
                    if command.llm_enabled
                    else FormationProposal(memories=(MemoryBody(text=unit.text),))
                )
                if not proposal.memories and not proposal.cognition:
                    raise DomainError("empty_formation", "Formation returned no cognitive output")
                async with self.authority.write(
                    ctx,
                    f"{command.operation_id}:{unit.id}:adopt",
                    operation_kind="imports.adopt",
                    scope=ctx.require_subject(),
                    payload={"command": command.model_dump(mode="json"), "unit_id": unit.id},
                ) as tx:
                    if tx.replayed:
                        output = UnitOutput.model_validate(tx.replay_result)
                    else:
                        await self.read_catalog_in(tx, ctx, command.catalog_id)
                        current_unit = (await tx.connection.execute(select(units.c.record).where(units.c.id == unit.id))).scalar_one()
                        if current_unit.get("purged"):
                            raise DomainError("source_purged", "Import input was purged while formation was in progress")
                        document = ImportDocument.model_validate(source["document"])
                        event = await self.materials.observe_in(tx, ctx, unit.material_id, document)
                        times = CognitiveTimes(
                            event=proposal.event or document.event_time,
                            observation=document.observed_at or ctx.clock,
                            formation=ctx.clock,
                            validity=proposal.validity,
                            recorded=ctx.clock,
                        )
                        refs: list[CognitiveRef] = []
                        for body in (*proposal.memories, *proposal.cognition):
                            record = await self.cognition.adopt_in(
                                tx,
                                ctx,
                                AdoptCognition(
                                    body=body,
                                    evidence=(EvidenceLink(observation_id=event["id"]),),
                                    times=times,
                                    producer_call_id=proposal.producer_call_id,
                                ),
                            )
                            refs.append(record.reference)
                        output = UnitOutput(
                            id=str(uuid4()),
                            unit_id=unit.id,
                            observation_id=event["id"],
                            cognition=tuple(refs),
                            attempt_job_id=attempt_job_id,
                            producer_call_id=proposal.producer_call_id,
                            vector_policy=command.vectors,
                            commit_seq=tx.commit_seq,
                        )
                        await tx.connection.execute(
                            outputs.insert().values(
                                id=output.id,
                                unit_id=unit.id,
                                operation_id=command.operation_id,
                                record=output.model_dump(mode="json"),
                            )
                        )
                        tx.result(output)
            adopted.append(output)
            if command.vectors != "disabled":
                await self.embed_output(ctx, output.id, required=command.vectors == "required")
        facts = await self.complete_files(ctx, command, catalog, adopted)
        return ImportResult(catalog=catalog, outputs=tuple(adopted), file_successes=tuple(facts))

    async def embed_output(self, ctx: CallContext, output_id: str, *, required: bool = True):
        async with self.authority.read(ctx) as view:
            value = (
                await view.connection.execute(
                    select(outputs.c.record).where(outputs.c.id == output_id)
                )
            ).scalar_one()
            output = UnitOutput.model_validate(value)
            observation = await require_observation_in(view, ctx, output.observation_id)
            records = [await self.cognition.get_in(view, ctx, ref) for ref in output.cognition]
        if not records:
            return {"output_id": output_id, "embedded": 0}
        batch = await self.models.embed(
            ctx.model_copy(update={"request_id": f"{output_id}:embed"}),
            tuple(body_text(record.body) for record in records),
            source_ids=(observation["material_id"],),
        )
        if len(batch.vectors) != len(records) or any(
            len(vector) != batch.dimensions or not all(math.isfinite(x) for x in vector)
            for vector in batch.vectors
        ):
            raise DomainError(
                "invalid_embedding",
                "Embedding response has invalid count, dimensions, or coordinates",
            )
        async with self.authority.write(
            ctx,
            f"{output_id}:embed:{batch.space}",
            operation_kind="imports.embed",
            scope=ctx.require_subject(),
            payload={"output_id": output_id, "space": batch.space, "model": batch.model},
        ) as tx:
            if tx.replayed:
                return tx.replay_result
            await require_observation_in(tx, ctx, output.observation_id)
            inserted = 0
            for record, vector in zip(records, batch.vectors, strict=True):
                existing = (await tx.connection.execute(select(vectors.c.model, vectors.c.dimensions).where(vectors.c.reference == record.reference.exact(), vectors.c.space == batch.space))).first()
                if existing is not None:
                    if existing.model != batch.model or existing.dimensions != batch.dimensions:
                        raise DomainError("embedding_space_conflict", "An embedding space cannot mix models or dimensions")
                    continue
                await tx.connection.execute(
                    vectors.insert().values(
                        reference=record.reference.exact(),
                        space=batch.space,
                        model=batch.model,
                        dimensions=batch.dimensions,
                        vector=list(vector),
                        call_id=batch.call_id,
                        commit_seq=tx.commit_seq,
                    )
                )
                inserted += 1
            return tx.result(
                {"output_id": output_id, "embedded": inserted, "space": batch.space}
            )

    async def complete_files(
        self,
        ctx: CallContext,
        command: ProcessCatalog,
        catalog: CatalogView,
        adopted: list[UnitOutput],
    ) -> list[FileSuccess]:
        by_unit = {output.unit_id: output for output in adopted}
        result: list[FileSuccess] = []
        async with self.authority.write(
            ctx,
            f"{command.operation_id}:complete",
            operation_kind="imports.complete",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                return TypeAdapter(list[FileSuccess]).validate_python(tx.replay_result)
            for source_id in catalog.input_source_ids:
                file_units = [unit for unit in catalog.units if unit.input_source_id == source_id]
                if not file_units or not all(unit.id in by_unit for unit in file_units):
                    continue
                source = (
                    await tx.connection.execute(
                        select(inputs.c.record).where(inputs.c.id == source_id)
                    )
                ).scalar_one()
                for unit in file_units:
                    await require_observation_in(tx, ctx, by_unit[unit.id].observation_id)
                document = ImportDocument.model_validate(source["document"])
                fact = FileSuccess(
                    id=str(uuid4()),
                    catalog_id=catalog.id,
                    input_source_id=source_id,
                    unit_outputs={unit.id: by_unit[unit.id].id for unit in file_units},
                    content_hash=source["content_hash"],
                    file_identity=document.file_identity,
                    origin=document.origin,
                    commit_seq=tx.commit_seq,
                )
                await tx.connection.execute(
                    successes.insert().values(
                        id=fact.id,
                        namespace_id=ctx.namespace_id,
                        subject_id=ctx.require_subject(),
                        input_source_id=source_id,
                        record=fact.model_dump(mode="json"),
                    )
                )
                result.append(fact)
            tx.result(result)
        return result

    async def admit_files(self, ctx: CallContext, command: AdmitFiles):
        receipt = await self.authority.receipt(
            ctx,
            command.operation_id,
            operation_kind="imports.manifest",
            scope=ctx.subject_id or "",
            payload=command,
        )
        if receipt is not None:
            return ImportResult.model_validate(receipt.result)
        selected: list[ImportDocument] = []
        skipped: list[str] = []
        async with self.authority.read(ctx) as view:
            await require_subject_in(view, ctx, "memory")
            for document in command.inputs:
                key = manifest_key(ctx, document, command.dedupe_policy)
                success_id = (
                    (
                        await view.connection.execute(
                            select(manifest.c.success_id).where(manifest.c.key == key)
                        )
                    ).scalar_one_or_none()
                    if command.dedupe_policy != "none"
                    and not command.force
                    and not command.clear_manifest
                    else None
                )
                usable = False
                if success_id:
                    fact = (
                        await view.connection.execute(
                            select(successes.c.record).where(
                                successes.c.id == success_id,
                                successes.c.namespace_id == ctx.namespace_id,
                                successes.c.subject_id == ctx.subject_id,
                            )
                        )
                    ).scalar_one_or_none()
                    if fact and fact["content_hash"] == sha256(document.text.encode()).hexdigest():
                        usable = True
                        for output_id in fact["unit_outputs"].values():
                            row = (
                                await view.connection.execute(
                                    select(outputs.c.record).where(outputs.c.id == output_id)
                                )
                            ).scalar_one_or_none()
                            try:
                                if row is None:
                                    usable = False
                                    break
                                await require_observation_in(view, ctx, row["observation_id"])
                                for ref in UnitOutput.model_validate(row).cognition:
                                    await self.cognition.get_in(view, ctx, ref)
                            except DomainError:
                                usable = False
                                break
                if usable and success_id is not None:
                    skipped.append(success_id)
                else:
                    selected.append(document)
        if not selected:
            return {"skipped_success_ids": skipped, "outputs": [], "file_successes": []}
        catalog = await self.freeze_catalog(
            ctx,
            FreezeCatalog(
                operation_id=f"{command.operation_id}:freeze",
                inputs=tuple(selected),
                chunk_size=command.chunk_size,
                chunk_overlap=command.chunk_overlap,
            ),
        )
        result = await self.process_catalog(
            ctx,
            ProcessCatalog(
                operation_id=f"{command.operation_id}:process",
                catalog_id=catalog.id,
                llm_enabled=command.llm_enabled,
                vectors=command.vectors,
            ),
        )
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="imports.manifest",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                return ImportResult.model_validate(tx.replay_result)
            for document, fact in zip(selected, result.file_successes, strict=True):
                if command.dedupe_policy != "none":
                    key = manifest_key(ctx, document, command.dedupe_policy)
                    await tx.connection.execute(delete(manifest).where(manifest.c.key == key))
                    await tx.connection.execute(
                        manifest.insert().values(key=key, success_id=fact.id)
                    )
            result = result.model_copy(update={"skipped_success_ids": tuple(skipped)})
            return tx.result(result)
