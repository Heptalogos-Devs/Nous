"""Form from an accepted source event without inventing another import or file job."""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING

from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.imports import FormObservation, ObservationFormation

from nous.material.observation import cognitive_times

if TYPE_CHECKING:
    from .service import ImportService


async def formation_result_in(self, view, ctx: CallContext, command: FormObservation):
    from sqlalchemy import select
    from .observation_jobs import formation_request_id
    from .schema import requests

    row = await view.connection.scalar(
        select(requests.c.record).where(requests.c.id == formation_request_id(ctx, command))
    )
    if row is None:
        return None
    if row["command"] != command.model_dump(mode="json"):
        raise DomainError(
            "formation_request_conflict", "Formation run has different accepted input"
        )
    if row.get("purged"):
        raise DomainError("source_purged", "Formation source is unavailable")
    raw = row.get("adoption_result")
    if raw is None:
        return None
    result = ObservationFormation.model_validate(raw)
    await self.materials.accepted_source_in(view, ctx, command.observation_id, command.source)
    for reference in result.cognition:
        await self.cognition.get_in(view, ctx, reference)
    return result


async def save_formation_in(
    self, tx, ctx: CallContext, command: FormObservation, result: ObservationFormation
):
    from sqlalchemy import select
    from sqlalchemy.dialects.sqlite import insert
    from .observation_jobs import formation_request_id
    from .schema import requests

    identity = formation_request_id(ctx, command)
    previous = await tx.connection.scalar(
        select(requests.c.record).where(requests.c.id == identity)
    )
    value = {
        **(previous or {}),
        "kind": "observation_form",
        "context": ctx.model_dump(mode="json"),
        "command": command.model_dump(mode="json"),
        "adoption_result": result.model_dump(mode="json"),
    }
    await tx.connection.execute(
        insert(requests)
        .values(id=identity, record=value)
        .on_conflict_do_update(index_elements=[requests.c.id], set_={"record": value})
    )
    if previous is None or previous.get("adoption_result") is None:
        from nous.material.service import require_observation_in
        from nous.platform.execution import JobSpec

        event = await require_observation_in(tx, ctx, command.observation_id)
        if event["kind"] == "contact" and result.cognition:
            await tx.enqueue(
                JobSpec(
                    "nous.maintenance.contact",
                    "default",
                    args=(
                        ctx.model_dump(mode="json"),
                        command.observation_id,
                        [reference.model_dump(mode="json") for reference in result.cognition],
                    ),
                    workflow_id="contact-formation:" + identity,
                    scope=ctx.namespace_id,
                )
            )
    return result


async def adopt_observation(
    self: ImportService, ctx: CallContext, command: FormObservation
) -> ObservationFormation:
    from nous.material.observation import cognitive_times
    from nous.material.text import source_region_text
    from nous.platform.content import ContentObject
    from nous.platform.persistence.authority import canonical_json
    from nous_sdk.contracts import EvidenceLink, MediaReference
    from nous_sdk.contracts.derivation import DerivationInput, StructuredRepresentation, TextRepresentation

    async with self.authority.read(ctx) as view:
        selected = await self.materials.accepted_source_in(
            view, ctx, command.observation_id, command.source
        )
        reference, material = selected.source, selected.material
        region_id = reference.id if reference.kind == "source_region" else None
        if reference.kind in {"representation", "derived_region"}:
            payload = (await self.cognition.derivations.selected_in(view, ctx, reference)).payload
            text = (
                payload.text
                if isinstance(payload, TextRepresentation)
                else canonical_json(payload.value)
                if isinstance(payload, StructuredRepresentation)
                else None
            )
        else:
            material, region, _ = await self.materials.media_source_in(view, ctx, reference)
            text = material["text"]
            if text is None:
                if self.content_store is None or not (
                    material["media_type"].startswith("text/")
                    or "json" in material["media_type"]
                ):
                    raise DomainError(
                        "media_formation_required", "Interpret the selected non-text media first"
                    )
                async with self.content_store.protect(
                    ContentObject(
                        material["content_hash"], material["byte_len"], material["media_type"]
                    )
                ) as path:
                    text = (await asyncio.to_thread(path.read_bytes)).decode("utf-8-sig")
            if region:
                text = source_region_text(text, region)
        if not text or not text.strip():
            raise DomainError("formation_source_empty", "The accepted source contains no text")
        catalog = (
            await self.formation.formation_catalog_in(view, ctx, text)
            if command.formation_purpose in {"graph", "memory_and_graph"}
            else ()
        )

    event = selected.event
    times = cognitive_times(event, ctx.clock, inferred_event=material.get("event_time"))
    source_inputs = (DerivationInput(ordinal=0, source=reference),)
    source_ids = (reference.exact(),)
    evidence = (EvidenceLink(observation_id=command.observation_id, region_id=region_id),)
    notes_result = None
    graph_result = None
    graph_plan = None
    graph_resolution = None
    if command.llm_enabled:
        from .model_generation import extract_graph, form_notes, resolve_graph

        if command.formation_purpose in {"memory", "memory_and_graph"}:
            notes_result = await form_notes(
                self.models,
                ctx,
                text,
                configuration=self.configuration,
                source_ids=source_ids,
            )
        if command.formation_purpose in {"graph", "memory_and_graph"}:
            graph_result = await extract_graph(
                self.models,
                ctx,
                text,
                configuration=self.configuration,
                source_ids=source_ids,
            )
            graph_plan = self.formation.graph_resolution_plan(
                graph_result.output, text, catalog
            )
            if graph_plan.request is not None:
                graph_resolution = await resolve_graph(
                    self.models, ctx, graph_plan.request, source_ids=source_ids
                )

    records = []
    representations = []
    async with self.authority.write(ctx) as tx:
        if notes_result is not None:
            adopted, representation = await self.formation.adopt_notes_result_in(
                tx,
                ctx,
                notes_result,
                observation_id=command.observation_id,
                region_id=region_id,
                times=times,
                inputs=source_inputs,
                evidence=evidence,
                media=(reference,),
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
                observation_id=command.observation_id,
                times=times,
                inputs=source_inputs,
                evidence=evidence,
                media=(reference,),
            )
            records.extend(adopted)
            representations.extend(graph_representations)
        references = tuple(record.reference for record in records)
        projection_targets = (
            *references,
            *(reference.media() for reference in representations),
        )
        projection = (
            await self.projection.admit_in(tx, ctx, projection_targets)
            if self.projection is not None
            and command.vectors != "disabled"
            and projection_targets
            else None
        )
        return ObservationFormation(
            observation_id=command.observation_id,
            cognition=references,
            representations=tuple(representations),
            projection=projection,
            vector_policy=command.vectors,
            commit_seq=tx.commit_seq,
        )


async def form_observation(
    self: ImportService, ctx: CallContext, command: FormObservation
) -> ObservationFormation:
    from httpx import HTTPError
    from openai import APIError

    from .attempts import failure
    from nous_sdk.contracts.derivation import ArtifactRepresentation

    async with self.authority.read(ctx) as view:
        saved = await formation_result_in(self, view, ctx, command)
        if saved is not None:
            return saved
        selected = await self.materials.accepted_source_in(
            view, ctx, command.observation_id, command.source
        )
        if selected.source.kind in {"representation", "derived_region"}:
            media_source = isinstance(
                (await self.cognition.derivations.selected_in(view, ctx, selected.source)).payload,
                ArtifactRepresentation,
            )
        else:
            media_source = selected.material["text"] is None and not (
                selected.material["media_type"].startswith("text/")
                or "json" in selected.material["media_type"]
            )
    if media_source:
        from .observation_media import form_media_observation

        result = await form_media_observation(self, ctx, command)
    else:
        result = await adopt_observation(self, ctx, command)
    vector_error = None
    if command.vectors != "disabled":
        try:
            if result.projection is not None and self.projection is not None:
                await self.projection.execution().wait(ctx, result.projection.job_id)
            elif result.cognition:
                await self.embed_records(ctx, result.cognition, command.operation_id + ":embed")
        except (DomainError, APIError, HTTPError, TimeoutError) as error:
            if command.vectors == "required" or (
                isinstance(error, DomainError)
                and error.code in {"source_unavailable", "source_purged", "subject_not_found"}
            ):
                raise
            vector_error = failure(error)
    result = result.model_copy(
        update={"vector_policy": command.vectors, "vector_error": vector_error}
    )
    async with self.authority.write(ctx) as tx:
        for reference in result.cognition:
            await self.cognition.get_in(tx, ctx, reference)
        return await save_formation_in(self, tx, ctx, command, result)
