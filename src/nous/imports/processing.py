from __future__ import annotations

from typing import TYPE_CHECKING
from uuid import uuid4

from httpx import HTTPError
from nous_wave.contracts import (
    CallContext,
    CatalogView,
    CognitiveTimes,
    DomainError,
    FileSuccess,
    FormationProposal,
    ImportDocument,
    ImportRecord,
    ImportResult,
    ImportUnit,
    ProcessCatalog,
    TimePoint,
    UnitOutput,
)
from openai import APIError
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select, update

from nous.cognition.formation_validation import validate_formation_draft
from nous.material.service import require_observation_in
from nous.platform.execution import JobSpec

from .attempts import AttemptRecorder, failure, record_attempt
from .schema import inputs, outputs, requests, successes, units

if TYPE_CHECKING:
    from .service import ImportService


async def submit_process(self: ImportService, ctx: CallContext, command: ProcessCatalog):
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
        request = (
            await tx.connection.execute(
                select(requests.c.record).where(requests.c.id == request_id)
            )
        ).scalar_one()
        request["job_id"] = job.workflow_id
        await tx.connection.execute(
            update(requests).where(requests.c.id == request_id).values(record=request)
        )
        return tx.result(
            {
                "request_id": request_id,
                "job_id": job.workflow_id,
                "catalog_id": command.catalog_id,
            }
        )


async def run_process(self: ImportService, request_id: str):
    async with self.authority.read() as view:
        row = (
            await view.connection.execute(
                select(requests.c.record).where(requests.c.id == request_id)
            )
        ).scalar_one()
    if row.get("purged"):
        raise DomainError("source_purged", "The import request's source was purged")
    if row.get("kind") == "files":
        result = await self.finish_admit(CallContext.model_validate(row["context"]), request_id)
        return {
            "catalog_id": result.catalog.id if result.catalog else None,
            "output_ids": [output.id for output in result.outputs],
            "file_success_ids": [fact.id for fact in result.file_successes],
            "skipped_success_ids": list(result.skipped_success_ids),
        }
    result = await self.process_catalog(
        CallContext.model_validate(row["context"]),
        ProcessCatalog.model_validate(row["command"]),
        attempt_job_id=row.get("job_id"),
    )
    return {
        "catalog_id": result.catalog.id if result.catalog else None,
        "output_ids": [output.id for output in result.outputs],
        "file_success_ids": [fact.id for fact in result.file_successes],
    }


async def process_catalog(
    self: ImportService,
    ctx: CallContext,
    command: ProcessCatalog,
    *,
    attempt_job_id: str | None = None,
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
        correction: dict[str, JsonValue] | None = None
        for retry in range(command.formation_retries + 1):
            try:
                async with record_attempt(
                    self, ctx, unit, command.operation_id, attempt_job_id
                ) as attempt:
                    output = await process_unit(
                        self,
                        ctx,
                        command,
                        catalog,
                        unit,
                        attempt_job_id,
                        attempt,
                        correction,
                        retry,
                    )
            except DomainError as error:
                if (
                    not command.llm_enabled
                    or retry >= command.formation_retries
                    or error.code
                    not in {
                        "formation_key_unknown",
                        "formation_key_conflict",
                        "formation_target_unknown",
                        "formation_cycle",
                    }
                ):
                    raise
                correction = {"code": error.code, "message": str(error), **error.details}
            else:
                adopted.append(output)
                break
    facts = await self.complete_files(ctx, command, catalog, adopted)
    return ImportResult(catalog=catalog, outputs=tuple(adopted), file_successes=tuple(facts))


async def complete_files(
    self: ImportService,
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
            if any(error.input_source_id == source_id for error in catalog.errors):
                continue
            file_units = [unit for unit in catalog.units if unit.input_source_id == source_id]
            if not file_units or not all(unit.id in by_unit for unit in file_units):
                continue
            source = (
                await tx.connection.execute(select(inputs.c.record).where(inputs.c.id == source_id))
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


async def process_unit(
    self: ImportService,
    ctx: CallContext,
    command: ProcessCatalog,
    catalog: CatalogView,
    unit: ImportUnit,
    attempt_job_id: str | None,
    attempt: AttemptRecorder,
    correction: dict[str, JsonValue] | None = None,
    retry: int = 0,
) -> UnitOutput:
    if unit.purged:
        raise DomainError("source_purged", "A purged import unit cannot be processed")
    async with self.authority.read(ctx) as view:
        if unit.observation_id:
            await require_observation_in(
                view, ctx, unit.observation_id, region_id=unit.source_region_id
            )
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
        source_record = ImportRecord.model_validate(
            {"external_id": f"unit:{unit.id}", "text": unit.text, **unit.context}
        )
        model_source: dict[str, object] = {
            "id": source["id"],
            "material_id": source["material_id"],
            "document": source["document"],
            "content_hash": source["content_hash"],
            "unit_context": unit.context,
            "strategy": unit.strategy,
            "split_context": unit.split_context,
            "formation_options": unit.formation_options,
            "formation_catalog": [
                item.model_dump(mode="json") for item in catalog.formation_catalog
            ],
        }
        if correction is not None:
            model_source["formation_correction"] = correction
        model_ctx = ctx.model_copy(
            update={"request_id": f"{command.operation_id}:{unit.id}:form:{retry}"}
        )
        await attempt.advance("formation")
        proposal = (
            (await self.models.form(model_ctx, unit.text, model_source))
            if command.llm_enabled and unit.requires_llm
            else FormationProposal(memories=())
        )
        if unit.formation_options.get("extract_time") is False:
            proposal = proposal.model_copy(update={"event": None, "event_expression": None})
        attempt.value = attempt.value.model_copy(
            update={"producer_call_id": proposal.producer_call_id}
        )
        try:
            validate_formation_draft(proposal, catalog.formation_catalog)
        except DomainError as error:
            error.details["previous_call_id"] = proposal.producer_call_id
            error.details["previous_proposal"] = proposal.model_dump(mode="json")
            raise
        await attempt.advance("adoption")
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
                current_unit = (
                    await tx.connection.execute(select(units.c.record).where(units.c.id == unit.id))
                ).scalar_one()
                if current_unit.get("purged"):
                    raise DomainError(
                        "source_purged",
                        "Import input was purged while formation was in progress",
                    )
                document = ImportDocument.model_validate(source["document"])
                document = document.model_copy(
                    update={
                        "event_time": source_record.event_time or document.event_time,
                        "observed_at": source_record.observed_at or document.observed_at,
                        "origin": {**document.origin, **source_record.origin},
                        "source_type": source_record.source_type or document.source_type,
                    }
                )
                event = (
                    (
                        await require_observation_in(
                            tx, ctx, unit.observation_id, region_id=unit.source_region_id
                        )
                    )["record"]
                    if unit.observation_id
                    else await self.materials.observe_in(tx, ctx, unit.material_id, document)
                )
                times = CognitiveTimes(
                    event=proposal.event or document.event_time,
                    observation=TimePoint.model_validate(event["observed_at"]),
                    formation=ctx.clock,
                    validity=proposal.validity,
                    recorded=ctx.clock,
                )
                records = await self.cognition.adopt_text_formation_in(
                    tx,
                    ctx,
                    unit.text,
                    source_record,
                    proposal,
                    event["id"],
                    unit.source_region_id,
                    times,
                    catalog.formation_catalog,
                )
                refs = [record.reference for record in records]
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
    attempt.value = attempt.value.model_copy(update={"output_id": output.id})
    if command.vectors != "disabled":
        await attempt.advance("embedding")
        try:
            await self.embed_output(ctx, output.id, required=command.vectors == "required")
        except (DomainError, APIError, HTTPError, TimeoutError) as error:
            attempt.value = attempt.value.model_copy(update={"vector_error": failure(error)})
            if command.vectors == "required" or (
                isinstance(error, DomainError)
                and error.code in {"source_unavailable", "source_purged", "subject_not_found"}
            ):
                raise
    return output
