from __future__ import annotations

from graphlib import CycleError, TopologicalSorter
from typing import TYPE_CHECKING
from uuid import uuid4

from httpx import HTTPError
from nous_sdk.contracts import (
    CatalogView,
    CognitiveRecord,
    FormObservation,
    ImportDocument,
    ImportRecord,
    ImportResult,
    ImportUnit,
    MediaReference,
    ProcessCatalog,
    UnitOutput,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.cognitive_identity import EvidenceLink
from nous_sdk.contracts.derivation import DerivationInput, RepresentationRef
from nous_sdk.contracts.imports import ImportUnitFailure
from openai import APIError
from pydantic import JsonValue
from sqlalchemy import select, update

from nous.material.observation import cognitive_times
from nous.material.service import require_observation_in
from nous.platform.execution import JobSpec

from .attempts import AttemptRecorder, failure, record_attempt
from .continuation import (
    formation_options,
    request_profiles_in,
    unit_candidates_in,
    validate_retry_in,
)
from .schema import inputs, outputs, requests, units
from .source_events import processing_source

if TYPE_CHECKING:
    from .service import ImportService


async def submit_process(self: ImportService, ctx: CallContext, command: ProcessCatalog):
    async with self.authority.write(ctx) as tx:
        await self.read_catalog_in(tx, ctx, command.catalog_id)
        await validate_retry_in(tx, ctx, command)
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
            JobSpec(workflow_name='nous.imports.process', queue_name='default' if command.deferred_formation else 'formation', args=(request_id,), scope=ctx.namespace_id)
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
        return {'request_id': request_id, 'job_id': job.workflow_id, 'catalog_id': command.catalog_id}


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
    async with self.authority.read(ctx) as view:
        await validate_retry_in(view, ctx, command)
        captured_profiles = await request_profiles_in(view, ctx, catalog.id)
    if not any((profile == command and row.get('job_id') == attempt_job_id and (CallContext.model_validate(row['context']).caller_id == ctx.caller_id) for row, profile in captured_profiles)):
        async with self.authority.write(ctx) as tx:
            await self.read_catalog_in(tx, ctx, command.catalog_id)
            await tx.connection.execute(requests.insert().values(id=str(uuid4()), record={'context': ctx.model_dump(mode='json'), 'command': command.model_dump(mode='json'), 'job_id': attempt_job_id}))
    selected = (
        set(command.unit_ids)
        if command.unit_ids is not None
        else {unit.id for unit in catalog.units}
    )
    if not selected.issubset({unit.id for unit in catalog.units}):
        raise DomainError("unit_not_found", "Requested unit does not belong to this catalog")
    adopted: list[UnitOutput] = []
    failures: list[ImportUnitFailure] = []
    for unit in ordered_units(catalog.units):
        if unit.id not in selected:
            continue
        try:
            async with record_attempt(
                self, ctx, unit, command.operation_id, attempt_job_id
            ) as attempt:
                async with self.authority.read(ctx) as view:
                    history = (
                        {}
                        if command.force
                        else await unit_candidates_in(
                            self, view, ctx, catalog, command, {unit.id}
                        )
                    )
                candidates = history.get(unit.id, [])
                completed = next((item for item in candidates if item.complete), None)
                if completed is not None:
                    output = completed.output
                    attempt.value = attempt.value.model_copy(
                        update={"output_id": output.id}
                    )
                    await attempt.advance("complete")
                else:
                    output = await process_unit(
                        self,
                        ctx,
                        command,
                        catalog,
                        unit,
                        attempt_job_id,
                        attempt,
                        reused=candidates[0].output if candidates else None,
                    )
        except DomainError as error:
            if len(failures) < command.max_errors and error.code not in {
                "source_unavailable",
                "source_purged",
                "namespace_disabled",
                "subject_disabled",
                "namespace_purging",
                "model_unavailable",
            }:
                failures.append(
                    ImportUnitFailure(
                        unit_id=unit.id,
                        code=error.code,
                        message=str(error),
                        details=error.details,
                    )
                )
                continue
            raise
        adopted.append(output)
    facts = await self.complete_files(ctx, command, catalog)
    return ImportResult(
        catalog=catalog,
        outputs=tuple(adopted),
        file_successes=tuple(facts),
        failures=tuple(failures),
    )


def ordered_units(catalog_units: tuple[ImportUnit, ...]) -> tuple[ImportUnit, ...]:
    """Declared source bindings are processed after their actual defining rows."""
    if not any("assertions" in unit.context for unit in catalog_units):
        return catalog_units
    records = {
        unit.id: ImportRecord.model_validate(
            {"external_id": "unit:" + unit.id, "text": unit.text, **unit.context}
        )
        for unit in catalog_units
        if unit.text
    }
    definitions: dict[tuple[str, str, str], set[str]] = {}
    for identifier, record in records.items():
        for key, value in {**record.identifiers, "import": record.external_id}.items():
            definitions.setdefault(("memory", key, value), set()).add(identifier)
        if record.assertions:
            for entity in record.assertions.entities:
                for key, value in entity.identifiers.items():
                    definitions.setdefault(("entity", key, value), set()).add(identifier)
            for association in record.assertions.associations:
                for key, value in association.external_ids.items():
                    definitions.setdefault(("association", key, value), set()).add(identifier)
    dependencies: dict[str, set[str]] = {unit.id: set() for unit in catalog_units}
    for identifier, record in records.items():
        if record.assertions:
            for reference in record.assertions.references:
                for value in (reference.identifier, *reference.aliases):
                    dependencies[identifier].update(
                        definitions.get((reference.kind, reference.identifier_key, value), ())
                    )
    try:
        ordered = tuple(TopologicalSorter(dependencies).static_order())
    except CycleError as error:
        raise DomainError(
            "source_definition_cycle", "Source rows contain cyclic unresolved object definitions"
        ) from error
    by_id = {unit.id: unit for unit in catalog_units}
    return tuple(by_id[identifier] for identifier in ordered)


async def process_unit(
    self: ImportService,
    ctx: CallContext,
    command: ProcessCatalog,
    catalog: CatalogView,
    unit: ImportUnit,
    attempt_job_id: str | None,
    attempt: AttemptRecorder,
    *,
    reused: UnitOutput | None = None,
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
    async with self.authority.read(ctx) as view:
        saved = await view.connection.scalar(
            select(outputs.c.record).where(
                outputs.c.unit_id == unit.id,
                outputs.c.operation_id == command.operation_id,
            )
        )
        if saved is not None:
            output = UnitOutput.model_validate(saved)
            await require_observation_in(view, ctx, output.observation_id)
            for reference in output.cognition:
                await self.cognition.get_in(view, ctx, reference)
            reused = output
    if reused is not None:
        output = reused
    elif unit.strategy == "media":
        from .media_units import process_media_unit

        output = await process_media_unit(self, ctx, command, unit, attempt_job_id, attempt)
    else:
        source_record = ImportRecord.model_validate(
            {"external_id": f"unit:{unit.id}", "text": unit.text, **unit.context}
        )
        source_document = ImportDocument.model_validate(source["document"])
        accepted_event, ctx = await processing_source(self, ctx, unit, command, source_document)
        source_record = source_record.model_copy(update={"origin": accepted_event["origin"]})
        effective_options = formation_options(unit, command)
        source_reference = (
            MediaReference(
                kind="source_region", id=unit.source_region_id, observation_id=accepted_event["id"]
            )
            if unit.source_region_id
            else unit.source.model_copy(update={"observation_id": accepted_event["id"]})
            if unit.source is not None
            else MediaReference(
                kind="material", id=unit.material_id, observation_id=accepted_event["id"]
            )
        )
        source_inputs = (DerivationInput(ordinal=0, source=source_reference),)
        evidence = (
            EvidenceLink(
                observation_id=accepted_event["id"], region_id=unit.source_region_id
            ),
        )
        model_source_ids = (source_reference.exact(),)
        notes_result = None
        graph_result = None
        graph_plan = None
        graph_resolution = None
        has_model_input = (
            source_record.assertions is None
            and command.llm_enabled
            and unit.requires_llm
            and not command.deferred_formation
        )
        if has_model_input:
            from nous.cognition.formation_models import extract_graph, form_notes, resolve_graph

            model_ctx = ctx.model_copy(
                update={"request_id": f"{command.operation_id}:{unit.id}:form"}
            )
            await attempt.advance("formation")
            if command.formation_purpose in {"memory", "memory_and_graph"}:
                notes_result = await form_notes(
                    self.models,
                    model_ctx,
                    unit.text,
                    configuration=self.configuration,
                    source_ids=model_source_ids,
                    cache_policy="none" if command.force else "shared",
                )
            if command.formation_purpose in {"graph", "memory_and_graph"}:
                graph_result = await extract_graph(
                    self.models,
                    model_ctx,
                    unit.text,
                    configuration=self.configuration,
                    source_ids=model_source_ids,
                    cache_policy="none" if command.force else "shared",
                )
                graph_plan = self.formation.graph_resolution_plan(
                    graph_result.output, unit.text, catalog.formation_catalog
                )
                if graph_plan.request is not None:
                    graph_resolution = await resolve_graph(
                        self.models,
                        model_ctx,
                        graph_plan.request,
                        source_ids=model_source_ids,
                    )
        await attempt.advance("adoption")
        async with self.authority.write(ctx) as tx:
            await self.read_catalog_in(tx, ctx, command.catalog_id)
            saved = await tx.connection.scalar(
                select(outputs.c.record).where(
                    outputs.c.unit_id == unit.id,
                    outputs.c.operation_id == command.operation_id,
                )
            )
            if saved is not None:
                output = UnitOutput.model_validate(saved)
                await require_observation_in(tx, ctx, output.observation_id)
                for reference in output.cognition:
                    await self.cognition.get_in(tx, ctx, reference)
                return output
            current_unit = (
                await tx.connection.execute(select(units.c.record).where(units.c.id == unit.id))
            ).scalar_one()
            if current_unit.get("purged"):
                raise DomainError(
                    "source_purged", "Import input was purged while formation was in progress"
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
                await require_observation_in(
                    tx, ctx, accepted_event["id"], region_id=unit.source_region_id
                )
            )["record"]
            from nous_sdk.contracts.common import utc_now

            times = cognitive_times(
                event,
                utc_now(),
                inferred_event=source_record.event_time or document.event_time,
            )
            records: list[CognitiveRecord] = []
            representations: list[RepresentationRef] = []
            if source_record.assertions is not None:
                records.extend(
                    await self.formation.adopt_manual_draft_in(
                        tx,
                        ctx,
                        source_record.assertions,
                        observation_id=event["id"],
                        region_id=unit.source_region_id,
                        times=times,
                        catalog=catalog.formation_catalog,
                        media=(source_reference,),
                        facts=source_record.assertions.facts,
                        source_references=source_record.assertions.references,
                    )
                )
            else:
                if notes_result is not None:
                    adopted, representation = await self.formation.adopt_notes_result_in(
                        tx,
                        ctx,
                        notes_result,
                        observation_id=event["id"],
                        region_id=unit.source_region_id,
                        times=times,
                        inputs=source_inputs,
                        evidence=evidence,
                        source_record=source_record,
                        media=(source_reference,),
                        extract_time=effective_options.get("extract_time") is not False,
                    )
                    records.extend(adopted)
                    representations.append(representation)
                if graph_result is not None and graph_plan is not None:
                    adopted, graph_representations = await self.formation.adopt_graph_result_in(
                        tx,
                        ctx,
                        graph_result,
                        resolution=graph_resolution,
                        plan=graph_plan,
                        observation_id=event["id"],
                        times=times,
                        inputs=source_inputs,
                        evidence=evidence,
                        media=(source_reference,),
                    )
                    records.extend(adopted)
                    representations.extend(graph_representations)
            references = tuple(record.reference for record in records)
            formation = (
                await self.submit_form_observation_in(
                    tx,
                    ctx,
                    FormObservation(
                        operation_id=f"{command.operation_id}:{unit.id}:form",
                        observation_id=event["id"],
                        formation_purpose=command.formation_purpose,
                        source=source_reference,
                        llm_enabled=command.llm_enabled and unit.requires_llm,
                        vectors=command.vectors,
                    ),
                    owned_parent=attempt_job_id,
                )
                if command.deferred_formation
                else None
            )
            projection_targets = (
                *references,
                *(reference.media() for reference in representations),
            )
            projection = (
                await self.projection.admit_in(tx, ctx, projection_targets)
                if self.projection is not None
                and projection_targets
                and command.vectors != "disabled"
                and not command.deferred_formation
                else None
            )
            output = UnitOutput(
                projection=projection,
                formation=formation,
                id=str(uuid4()),
                unit_id=unit.id,
                observation_id=event["id"],
                cognition=references,
                representations=tuple(representations),
                attempt_job_id=attempt_job_id,
                vector_policy="disabled" if command.deferred_formation else command.vectors,
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
    attempt.value = attempt.value.model_copy(update={"output_id": output.id})
    if command.vectors != "disabled" and not command.deferred_formation and output.projection:
        await attempt.advance("embedding")
        try:
            await self.embed_output(
                ctx,
                output.id,
                required=command.vectors == "required",
                operation_id=f"{command.operation_id}:{unit.id}:vectors",
                receipt=output.projection if reused is None else None,
            )
        except (DomainError, APIError, HTTPError, TimeoutError) as error:
            attempt.value = attempt.value.model_copy(update={"vector_error": failure(error)})
            if command.vectors == "required" or (
                isinstance(error, DomainError)
                and error.code in {"source_unavailable", "source_purged", "subject_not_found"}
            ):
                raise
    return output
