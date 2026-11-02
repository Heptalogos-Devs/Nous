"""Form from an accepted source event without inventing another import or file job."""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    CallContext,
    DomainError,
    FormationProposal,
    ImportRecord,
    MemoryBody,
)
from nous_sdk.contracts.imports import FormObservation, ObservationFormation

from nous.cognition.formation_validation import validate_formation_draft
from nous.material.observation import cognitive_times
from nous.material.text import source_region_text
from nous.platform.content import ContentObject

from .model_generation import form
from .observation_anchor import anchor_in

if TYPE_CHECKING:
    from .service import ImportService


async def adopt_observation(
    self: ImportService, ctx: CallContext, command: FormObservation
) -> ObservationFormation:
    receipt = await self.authority.receipt(
        ctx,
        command.operation_id,
        operation_kind="cognition.form_observation",
        scope=ctx.namespace_id,
        payload=command.identity_payload(),
    )
    if receipt is not None:
        return ObservationFormation.model_validate(receipt.result)
    async with self.authority.read(ctx) as view:
        region_id = (
            command.source.id if command.source and command.source.kind == "source_region" else None
        )
        anchor = (
            await anchor_in(self, view, ctx, command, region_id=region_id)
            if command.source is None or command.anchor
            else None
        )
        if anchor is not None and command.source is None:
            exact_regions = {
                link.region_id
                for link in anchor.evidence
                if link.observation_id == command.observation_id
            }
            if len(exact_regions) != 1:
                raise DomainError(
                    "observation_anchor_ambiguous",
                    "The anchor requires an explicit exact source selection",
                )
            region_id = next(iter(exact_regions))
        from nous_sdk.contracts.derivation import StructuredRepresentation, TextRepresentation

        from nous.platform.persistence.authority import canonical_json

        selected = await self.materials.accepted_source_in(
            view,
            ctx,
            command.observation_id,
            command.source,
            derivations=self.cognition.derivations,
        )
        reference, material = selected.source, selected.material
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
                    material["media_type"].startswith("text/") or "json" in material["media_type"]
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
        if not text:
            raise DomainError("formation_source_empty", "The accepted source contains no text")
        catalog = await self.formation.formation_catalog_in(view, ctx, text)
        if anchor is None:
            anchor = await anchor_in(self, view, ctx, command, region_id=region_id, text=text)
        elif not isinstance(anchor.body, MemoryBody) or anchor.body.text != text:
            raise DomainError(
                "observation_anchor_mismatch",
                "The literal anchor differs from its exact selected source text",
            )
    if not text:
        raise DomainError("formation_source_empty", "The accepted source contains no text")
    source: dict[str, object] = {
        "material_id": material["id"],
        "observation_id": command.observation_id,
        "source_material_ids": sorted(
            {material["id"], *(item for entry in catalog for item in entry.material_ids)}
        ),
        "source": reference.model_dump(mode="json"),
        "formation_catalog": [entry.model_dump(mode="json") for entry in catalog],
    }
    proposal = FormationProposal(memories=())
    for attempt in range(2):
        proposal = (
            await form(
                self.models,
                ctx.model_copy(update={"request_id": command.operation_id + f":form:{attempt}"}),
                text,
                source,
                configuration=self.configuration,
            )
            if command.llm_enabled
            else FormationProposal(memories=())
        )
        try:
            validate_formation_draft(proposal, catalog)
        except DomainError as error:
            if attempt or error.code not in {
                "formation_key_unknown",
                "formation_key_conflict",
                "formation_target_unknown",
                "formation_cycle",
            }:
                raise
            source["formation_correction"] = {
                "code": error.code,
                "message": str(error),
                **error.details,
                "previous_call_id": proposal.producer_call_id,
                "previous_proposal": proposal.model_dump(mode="json"),
            }
        else:
            break
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="cognition.form_observation",
        scope=ctx.namespace_id,
        payload=command.identity_payload(),
    ) as tx:
        if tx.replayed:
            return ObservationFormation.model_validate(tx.replay_result)
        current = (
            await self.materials.accepted_source_in(
                tx,
                ctx,
                command.observation_id,
                command.source,
                derivations=self.cognition.derivations,
            )
        ).event
        times = cognitive_times(
            current, ctx.clock, inferred_event=proposal.event, inferred_validity=proposal.validity
        )
        if self.projection is not None:
            await self.projection.policy_in(tx, "disabled")
        if anchor is not None:
            current_anchor = await self.cognition.get_in(tx, ctx, anchor.reference)
            records = (
                current_anchor,
                *await self.formation.adopt_semantic_proposal_in(
                    tx, ctx, current_anchor, proposal, times, catalog
                ),
            )
        else:
            records = await self.formation.adopt_text_formation_in(
                tx,
                ctx,
                text,
                ImportRecord(
                    external_id="observation:" + command.observation_id,
                    text=text,
                    origin=current["origin"],
                    source_type=current["source_type"],
                ),
                proposal,
                command.observation_id,
                region_id,
                times,
                catalog,
            )
        return tx.result(
            ObservationFormation(
                projection=await self.projection.admit_in(
                    tx, ctx, tuple(record.reference for record in records)
                )
                if self.projection is not None and command.vectors != "disabled"
                else None,
                observation_id=command.observation_id,
                cognition=tuple(record.reference for record in records),
                producer_call_id=proposal.producer_call_id,
                commit_seq=tx.commit_seq,
            )
        )


async def form_observation(
    self: ImportService, ctx: CallContext, command: FormObservation
) -> ObservationFormation:
    from httpx import HTTPError
    from openai import APIError

    from .attempts import failure

    receipt = await self.authority.receipt(
        ctx,
        command.operation_id + ":complete",
        operation_kind="cognition.form_observation.complete",
        scope=ctx.namespace_id,
        payload=command.identity_payload(),
    )
    if receipt is not None:
        result = ObservationFormation.model_validate(receipt.result)
        async with self.authority.read(ctx) as view:
            for reference in result.cognition:
                await self.cognition.get_in(view, ctx, reference)
        return result
    from nous_sdk.contracts.derivation import ArtifactRepresentation

    async with self.authority.read(ctx) as view:
        selected = await self.materials.accepted_source_in(
            view,
            ctx,
            command.observation_id,
            command.source,
            derivations=self.cognition.derivations,
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
            else:
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
    async with self.authority.write(
        ctx,
        command.operation_id + ":complete",
        operation_kind="cognition.form_observation.complete",
        scope=ctx.namespace_id,
        payload=command.identity_payload(),
    ) as tx:
        if tx.replayed:
            return ObservationFormation.model_validate(tx.replay_result)
        for reference in result.cognition:
            await self.cognition.get_in(tx, ctx, reference)
        return tx.result(result)
