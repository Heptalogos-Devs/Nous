from __future__ import annotations

import json
from hashlib import sha256
from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    AdmitFiles,
    CatalogView,
    FreezeCatalog,
    ImportDocument,
    ImportResult,
    ProcessCatalog,
    UnitOutput,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from sqlalchemy import delete, select, update

from nous.cognition.schema import revisions, vectors
from nous.identity.access import require_namespace_in
from nous.material.service import require_observation_in
from nous.platform.content import ContentLease
from nous.platform.execution import JobSpec
from nous.platform.persistence import ReadView, WriteTx

from .schema import inputs, manifest, outputs, requests, successes, units

if TYPE_CHECKING:
    from .service import ImportService


def manifest_key(
    ctx: CallContext,
    document: ImportDocument,
    policy: str,
    content_hash: str | None = None,
    *,
    deferred_formation: bool = False,
    llm_enabled: bool = True,
) -> str:
    digest = content_hash or sha256(document.inline_text().encode()).hexdigest()
    key = document.file_identity if policy == "manifest" and document.file_identity else digest
    temporal = document.model_dump(
        mode="json", include={"event_time", "observed_at", "validity"}, exclude_unset=True
    )
    return sha256(
        json.dumps(
            [ctx.namespace_id, ctx.namespace_id, document.origin, policy, key]
            + ([{"times": temporal}] if temporal else [])
            + (
                [{"deferred_formation": True, "llm_enabled": llm_enabled}]
                if deferred_formation
                else []
            ),
            sort_keys=True,
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
    deferred_formation: bool = False,
    llm_enabled: bool = True,
) -> str | None:
    if policy == "none":
        return None
    key = manifest_key(
        ctx,
        document,
        policy,
        digest,
        deferred_formation=deferred_formation,
        llm_enabled=llm_enabled,
    )
    success_id = (
        await view.connection.execute(select(manifest.c.success_id).where(manifest.c.key == key))
    ).scalar_one_or_none()
    if success_id is None:
        return None
    fact = (
        await view.connection.execute(
            select(successes.c.record).where(
                successes.c.id == success_id, successes.c.namespace_id == ctx.namespace_id
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
        if deferred_formation and output.formation is None:
            return None
        if not deferred_formation and llm_enabled and not output.representations:
            original_unit = await view.connection.scalar(
                select(units.c.record).where(units.c.id == output.unit_id)
            )
            if original_unit is None or original_unit.get("requires_llm", True):
                return None
        try:
            await require_observation_in(view, ctx, output.observation_id)
            for reference in output.cognition:
                record = await self.cognition.get_in(view, ctx, reference)
                current = (
                    await view.connection.execute(
                        select(revisions.c.record).where(
                            revisions.c.namespace_id == ctx.namespace_id,
                            revisions.c.id == reference.id,
                            revisions.c.head.is_(True),
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
                            .where(
                                vectors.c.namespace_id == ctx.namespace_id,
                                vectors.c.reference == reference.exact(),
                            )
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
    self: ImportService,
    ctx: CallContext,
    command: AdmitFiles,
    *,
    enqueue: bool,
    attempt_job_id: str | None = None,
    parent_request_id: str | None = None,
):
    request_id = (
        "files:"
        + sha256(f"{ctx.caller_id}:{ctx.namespace_id}:{command.operation_id}".encode()).hexdigest()
    )
    submitted_digest = sha256(
        json.dumps(command.model_dump(mode="json"), sort_keys=True).encode()
    ).hexdigest()
    operation_id = command.operation_id if enqueue else f"{command.operation_id}:accept"
    prepared = [await self.prepare_document(ctx, document) for document in command.inputs]
    from .document_extraction import chunks_for

    split = [[chunks_for(parsed, command) for parsed in item.records] for item in prepared]
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
        async with self.authority.write(ctx) as tx:
            existing = await tx.connection.scalar(
                select(requests.c.record).where(requests.c.id == request_id)
            )
            if existing is not None:
                if existing.get("submitted_digest") != submitted_digest:
                    raise DomainError(
                        "import_request_conflict", "Import request identity has different input"
                    )
                effective = await self.input_context_in(tx, ctx, existing)
                if existing.get("catalog_id"):
                    await self.read_catalog_in(tx, effective, existing["catalog_id"])
                return {
                    "request_id": request_id,
                    "catalog_id": existing.get("catalog_id"),
                    "job_id": existing.get("job_id"),
                    "skipped_success_ids": existing["skipped_success_ids"],
                }
            await require_namespace_in(tx, ctx, "memory")
            if command.clear_manifest:
                origins = [document.origin for document in command.inputs]
                all_heads = (
                    (
                        await tx.connection.execute(
                            select(manifest.c.key, successes.c.record)
                            .join(successes, successes.c.id == manifest.c.success_id)
                            .where(successes.c.namespace_id == ctx.namespace_id)
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
            effective = ctx
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
                        tx,
                        ctx,
                        item.document,
                        digest,
                        command.dedupe_policy,
                        "disabled" if command.deferred_formation else command.vectors,
                        command.deferred_formation,
                        command.llm_enabled,
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
                frozen_command = FreezeCatalog.model_validate(
                    command.model_dump(include=set(FreezeCatalog.model_fields))
                ).model_copy(
                    update={
                        "operation_id": operation_id,
                        "inputs": tuple(prepared[index].document for index in selected),
                    }
                )
                accepted_inputs = None
                if parent_request_id is not None:
                    parent = await tx.connection.scalar(
                        select(requests.c.record).where(requests.c.id == parent_request_id)
                    )
                    if (
                        parent is None
                        or CallContext.model_validate(parent["context"]).caller_id != ctx.caller_id
                    ):
                        raise DomainError(
                            "import_request_not_found", "Parent import request is unavailable"
                        )
                    accepted_inputs = parent.get("input_observations")
                frozen = await self.freeze_catalog_in(
                    tx,
                    ctx,
                    frozen_command,
                    [leases[index] for index in selected],
                    [prepared[index] for index in selected],
                    [split[index] for index in selected],
                    admitted_observations=accepted_inputs,
                )
                catalog = frozen.catalog
                effective = frozen.context
                material_ids.extend(unit.material_id for unit in catalog.units)
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
                    deferred_formation=command.deferred_formation,
                    llm_enabled=command.llm_enabled,
                )
                for source_id, index in zip(
                    catalog.input_source_ids if catalog else (), selected, strict=True
                )
            }
            request = {
                "kind": "files",
                "submitted_digest": submitted_digest,
                "original_material_ids": [
                    item.material["id"] for item in prepared if item.material
                ],
                "input_observation_ids": list(
                    dict.fromkeys(
                        unit.observation_id for unit in catalog.units if unit.observation_id
                    )
                )
                if catalog
                else [],
                "context": effective.model_dump(mode="json"),
                "command": work_command.model_dump(mode="json", exclude={"inputs"}),
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
                        queue_name="default" if command.deferred_formation else "formation",
                        args=(request_id,),
                        scope=ctx.namespace_id,
                    )
                )
                if enqueue
                else None
            )
            request["job_id"] = job.workflow_id if job else attempt_job_id
            await tx.connection.execute(
                update(requests).where(requests.c.id == request_id).values(record=request)
            )
            if parent_request_id is not None:
                parent = await tx.connection.scalar(
                    select(requests.c.record).where(requests.c.id == parent_request_id)
                )
                if parent is None or parent.get("kind") != "text_import":
                    raise DomainError(
                        "import_request_not_found", "Text import parent is unavailable"
                    )
                parent_context = CallContext.model_validate(parent["context"])
                if (
                    parent_context.caller_id,
                    parent_context.namespace_id,
                    parent_context.namespace_id,
                ) != (ctx.caller_id, ctx.namespace_id, ctx.namespace_id):
                    raise DomainError(
                        "import_request_not_found",
                        "Text import parent is outside this caller scope",
                    )
                await tx.connection.execute(
                    update(requests)
                    .where(requests.c.id == parent_request_id)
                    .values(
                        record={
                            **parent,
                            "admission_request_id": request_id,
                            "context": effective.model_dump(mode="json"),
                            "material_ids": request["material_ids"],
                        }
                    )
                )
            return {'request_id': request_id, 'catalog_id': catalog.id if catalog else None, 'job_id': job.workflow_id if job else attempt_job_id, 'skipped_success_ids': skipped}
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
        ctx = await self.input_context_in(view, ctx, value)
    if value.get("purged"):
        raise DomainError("source_purged", "The import request's source was purged")
    command = value["command"]
    if value.get("result") is not None:
        return await self.read_admit_result(ctx, request_id)
    if value["catalog_id"]:
        result = await self.process_catalog(
            ctx,
            ProcessCatalog(
                operation_id=command["operation_id"] + ":process",
                force=command["force"],
                catalog_id=value["catalog_id"],
                llm_enabled=command["llm_enabled"],
                deferred_formation=command["deferred_formation"],
                formation_purpose=command["formation_purpose"],
                vectors=command["vectors"],
            ),
            attempt_job_id=value.get("job_id"),
        )
    else:
        result = ImportResult(catalog=None, outputs=(), file_successes=())
    async with self.authority.write(ctx) as tx:
        for output in result.outputs:
            await require_observation_in(tx, ctx, output.observation_id)
        for fact in result.file_successes:
            if command["dedupe_policy"] != "none":
                key = value["manifest_keys"][fact.input_source_id]
                await tx.connection.execute(delete(manifest).where(manifest.c.key == key))
                await tx.connection.execute(manifest.insert().values(key=key, success_id=fact.id))
        result = result.model_copy(
            update={"skipped_success_ids": tuple(value["skipped_success_ids"])}
        )
        current = await tx.connection.scalar(
            select(requests.c.record).where(requests.c.id == request_id)
        )
        await tx.connection.execute(
            update(requests)
            .where(requests.c.id == request_id)
            .values(
                record={
                    **current,
                    "result": {
                        **result.model_dump(
                            mode="json", exclude={"catalog", "outputs", "file_successes"}
                        ),
                        "output_ids": [output.id for output in result.outputs],
                        "file_success_ids": [fact.id for fact in result.file_successes],
                    },
                }
            )
        )
        return result


async def admit_files(self: ImportService, ctx: CallContext, command: AdmitFiles) -> ImportResult:
    accepted = await self.accept_admit(ctx, command, enqueue=False)
    return await self.finish_admit(ctx, str(accepted["request_id"]))


async def submit_admit_files(self: ImportService, ctx: CallContext, command: AdmitFiles):
    return await self.accept_admit(ctx, command, enqueue=True)
