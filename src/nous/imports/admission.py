from __future__ import annotations

import json
from hashlib import sha256
from typing import TYPE_CHECKING
from uuid import uuid4

from nous_wave.contracts import (
    AdmitFiles,
    CallContext,
    CatalogView,
    DomainError,
    FreezeCatalog,
    ImportDocument,
    ImportResult,
    ProcessCatalog,
    UnitOutput,
)
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import delete, select, update

from nous.cognition.schema import revisions, vectors
from nous.identity.service import require_subject_in
from nous.material.service import require_observation_in
from nous.platform.content import ContentLease
from nous.platform.execution import JobSpec
from nous.platform.persistence import ReadView, WriteTx

from .schema import inputs, manifest, outputs, requests, successes
from .splitting import split_text

if TYPE_CHECKING:
    from .service import ImportService


def manifest_key(
    ctx: CallContext, document: ImportDocument, policy: str, content_hash: str | None = None
) -> str:
    digest = content_hash or sha256(document.inline_text().encode()).hexdigest()
    key = document.file_identity if policy == "manifest" and document.file_identity else digest
    return sha256(
        json.dumps(
            [ctx.namespace_id, ctx.subject_id, document.origin, policy, key], sort_keys=True
        ).encode()
    ).hexdigest()


async def eligible_file_in(
    self: ImportService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    document: ImportDocument,
    digest: str,
    policy: str,
    vector_policy: str,
) -> str | None:
    if policy == "none":
        return None
    key = manifest_key(ctx, document, policy, digest)
    success_id = (
        await view.connection.execute(select(manifest.c.success_id).where(manifest.c.key == key))
    ).scalar_one_or_none()
    if success_id is None:
        return None
    fact = (
        await view.connection.execute(
            select(successes.c.record).where(
                successes.c.id == success_id,
                successes.c.namespace_id == ctx.namespace_id,
                successes.c.subject_id == ctx.subject_id,
            )
        )
    ).scalar_one_or_none()
    if fact is None or fact["content_hash"] != digest:
        return None
    original = (
        await view.connection.execute(
            select(inputs.c.record).where(inputs.c.id == fact["input_source_id"])
        )
    ).scalar_one()
    if original["document"].get("format", "text") != document.format:
        return None
    for output_id in fact["unit_outputs"].values():
        value = (
            await view.connection.execute(select(outputs.c.record).where(outputs.c.id == output_id))
        ).scalar_one_or_none()
        if value is None:
            return None
        output = UnitOutput.model_validate(value)
        try:
            await require_observation_in(view, ctx, output.observation_id)
            for reference in output.cognition:
                record = await self.cognition.get_in(view, ctx, reference)
                current = (
                    await view.connection.execute(
                        select(revisions.c.record).where(
                            revisions.c.id == reference.id, revisions.c.head.is_(True)
                        )
                    )
                ).scalar_one()
                if record.purged or current.get("purged") or current.get("deleted"):
                    return None
                if (
                    vector_policy == "required"
                    and (
                        await view.connection.execute(
                            select(vectors.c.reference)
                            .where(vectors.c.reference == reference.exact())
                            .limit(1)
                        )
                    ).first()
                    is None
                ):
                    return None
        except DomainError:
            return None
    return str(success_id)


async def accept_admit(
    self: ImportService, ctx: CallContext, command: AdmitFiles, *, enqueue: bool
):
    operation_id = command.operation_id if enqueue else f"{command.operation_id}:accept"
    operation_kind = "imports.files.submit" if enqueue else "imports.files.accept"
    receipt = await self.authority.receipt(
        ctx,
        operation_id,
        operation_kind=operation_kind,
        scope=ctx.require_subject(),
        payload=command,
    )
    if receipt is not None:
        return TypeAdapter(dict[str, JsonValue]).validate_python(receipt.result)
    prepared = [await self.prepare_document(ctx, document) for document in command.inputs]
    split = [
        [
            split_text(parsed.record.text, command) if parsed.record else []
            for parsed in item.records
        ]
        for item in prepared
    ]
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
        async with self.authority.write(
            ctx,
            operation_id,
            operation_kind=operation_kind,
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                return TypeAdapter(dict[str, JsonValue]).validate_python(tx.replay_result)
            await require_subject_in(tx, ctx, "memory")
            if command.clear_manifest:
                origins = [document.origin for document in command.inputs]
                all_heads = (
                    (
                        await tx.connection.execute(
                            select(manifest.c.key, successes.c.record)
                            .join(successes, successes.c.id == manifest.c.success_id)
                            .where(
                                successes.c.namespace_id == ctx.namespace_id,
                                successes.c.subject_id == ctx.subject_id,
                            )
                        )
                    )
                    .mappings()
                    .all()
                )
                for head in all_heads:
                    if head["record"]["origin"] in origins:
                        await tx.connection.execute(
                            delete(manifest).where(manifest.c.key == head["key"])
                        )
            selected: list[int] = []
            skipped: list[str] = []
            material_ids: list[str] = []
            for index, item in enumerate(prepared):
                digest = (
                    item.material["content_hash"]
                    if item.material
                    else sha256(item.content.encode()).hexdigest()
                )
                if item.material:
                    await self.materials.read_in(tx, ctx, item.material["id"])
                    material_ids.append(item.material["id"])
                success_id = (
                    None
                    if command.force or command.clear_manifest
                    else await self.eligible_file_in(
                        tx, ctx, item.document, digest, command.dedupe_policy, command.vectors
                    )
                )
                if success_id is None:
                    selected.append(index)
                else:
                    skipped.append(success_id)
                    old_source = (
                        await tx.connection.execute(
                            select(inputs.c.material_id)
                            .join(successes, successes.c.input_source_id == inputs.c.id)
                            .where(successes.c.id == success_id)
                        )
                    ).scalar_one()
                    material_ids.append(str(old_source))
            catalog: CatalogView | None = None
            if selected:
                frozen_command = FreezeCatalog(
                    operation_id=operation_id,
                    inputs=tuple(prepared[index].document for index in selected),
                    chunk_size=command.chunk_size,
                    chunk_overlap=command.chunk_overlap,
                    parent_catalog_id=command.parent_catalog_id,
                )
                catalog = await self.freeze_catalog_in(
                    tx,
                    ctx,
                    frozen_command,
                    [leases[index] for index in selected],
                    [prepared[index] for index in selected],
                    [split[index] for index in selected],
                )
                material_ids.extend(unit.material_id for unit in catalog.units)
            request_id = str(uuid4())
            work_command = (
                command.model_copy(update={"operation_id": f"{command.operation_id}:files"})
                if enqueue
                else command
            )
            keys = {
                source_id: manifest_key(
                    ctx,
                    prepared[index].document,
                    command.dedupe_policy,
                    prepared[index].content_hash,
                )
                for source_id, index in zip(
                    catalog.input_source_ids if catalog else (), selected, strict=True
                )
            }
            request = {
                "kind": "files",
                "context": ctx.model_dump(mode="json"),
                "command": work_command.model_dump(mode="json"),
                "catalog_id": catalog.id if catalog else None,
                "manifest_keys": keys,
                "skipped_success_ids": skipped,
                "material_ids": list(dict.fromkeys(material_ids)),
            }
            await tx.connection.execute(requests.insert().values(id=request_id, record=request))
            job = (
                await tx.enqueue(
                    JobSpec(
                        workflow_name="nous.imports.process",
                        queue_name="formation",
                        args=(request_id,),
                    )
                )
                if enqueue
                else None
            )
            request["job_id"] = job.workflow_id if job else None
            await tx.connection.execute(
                update(requests).where(requests.c.id == request_id).values(record=request)
            )
            return tx.result(
                {
                    "request_id": request_id,
                    "catalog_id": catalog.id if catalog else None,
                    "job_id": job.workflow_id if job else None,
                    "skipped_success_ids": skipped,
                }
            )
    finally:
        if self.content_store:
            for lease in leases:
                if lease is not None:
                    await self.content_store.release(lease)


async def finish_admit(self: ImportService, ctx: CallContext, request_id: str) -> ImportResult:
    async with self.authority.read(ctx) as view:
        value = (
            await view.connection.execute(
                select(requests.c.record).where(requests.c.id == request_id)
            )
        ).scalar_one()
    if value.get("purged"):
        raise DomainError("source_purged", "The import request's source was purged")
    command = AdmitFiles.model_validate(value["command"])
    receipt = await self.authority.receipt(
        ctx,
        command.operation_id,
        operation_kind="imports.manifest",
        scope=ctx.require_subject(),
        payload=command,
    )
    if receipt is not None:
        return ImportResult.model_validate(receipt.result)
    if value["catalog_id"]:
        result = await self.process_catalog(
            ctx,
            ProcessCatalog(
                operation_id=f"{command.operation_id}:process",
                catalog_id=value["catalog_id"],
                llm_enabled=command.llm_enabled,
                formation_retries=command.formation_retries,
                vectors=command.vectors,
            ),
            attempt_job_id=value.get("job_id"),
        )
    else:
        result = ImportResult(catalog=None, outputs=(), file_successes=())
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="imports.manifest",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            return ImportResult.model_validate(tx.replay_result)
        for output in result.outputs:
            await require_observation_in(tx, ctx, output.observation_id)
        for fact in result.file_successes:
            if command.dedupe_policy != "none":
                key = value["manifest_keys"][fact.input_source_id]
                await tx.connection.execute(delete(manifest).where(manifest.c.key == key))
                await tx.connection.execute(manifest.insert().values(key=key, success_id=fact.id))
        return tx.result(
            result.model_copy(update={"skipped_success_ids": tuple(value["skipped_success_ids"])})
        )


async def admit_files(self: ImportService, ctx: CallContext, command: AdmitFiles) -> ImportResult:
    receipt = await self.authority.receipt(
        ctx,
        command.operation_id,
        operation_kind="imports.manifest",
        scope=ctx.require_subject(),
        payload=command,
    )
    if receipt is not None:
        return ImportResult.model_validate(receipt.result)
    accepted = await self.accept_admit(ctx, command, enqueue=False)
    return await self.finish_admit(ctx, str(accepted["request_id"]))


async def submit_admit_files(self: ImportService, ctx: CallContext, command: AdmitFiles):
    return await self.accept_admit(ctx, command, enqueue=True)
