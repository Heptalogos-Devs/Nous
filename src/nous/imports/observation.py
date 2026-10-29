"""Form from an accepted source event without inventing another import or file job."""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING

from nous_wave.contracts import (
    CallContext,
    CognitiveTimes,
    DomainError,
    FormationProposal,
    ImportRecord,
    MediaReference,
    MemoryBody,
    TimePoint,
)
from nous_wave.contracts.imports import FormObservation, ObservationFormation

from nous.cognition.formation_validation import validate_formation_draft
from nous.material.service import require_observation_in
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
        scope=ctx.require_subject(),
        payload=command,
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
        event = await require_observation_in(view, ctx, command.observation_id, region_id=region_id)
        reference = command.source or MediaReference(
            kind="source_region" if region_id else "material", id=region_id or event["material_id"]
        )
        if (
            reference.observation_id is not None
            and reference.observation_id != command.observation_id
        ):
            raise DomainError(
                "observation_source_mismatch",
                "The selected source belongs to a different observation",
            )
        material, region, _ = await self.materials.media_source_in(
            view, ctx, reference.model_copy(update={"observation_id": command.observation_id})
        )
        if material["id"] != event["material_id"]:
            raise DomainError(
                "observation_source_mismatch", "The selected source belongs to a different original"
            )
        text = material["text"]
        if text is None:
            if self.content_store is None or not (
                material["media_type"].startswith("text/") or "json" in material["media_type"]
            ):
                raise DomainError(
                    "media_formation_required",
                    "Non-text observations require the media interpretation owner",
                )
            async with self.content_store.protect(
                ContentObject(
                    material["content_hash"], material["byte_len"], material["media_type"]
                )
            ) as path:
                text = (await asyncio.to_thread(path.read_bytes)).decode("utf-8-sig")
        if region:
            text = source_region_text(text, region)
        catalog = await self.cognition.formation_catalog_in(view, ctx, text)
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
    proposal = (
        await form(
            self.models,
            ctx.model_copy(update={"request_id": command.operation_id + ":form"}),
            text,
            source,
        )
        if command.llm_enabled
        else FormationProposal(memories=())
    )
    validate_formation_draft(proposal, catalog)
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="cognition.form_observation",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            return ObservationFormation.model_validate(tx.replay_result)
        current = await require_observation_in(tx, ctx, command.observation_id, region_id=region_id)
        times = CognitiveTimes(
            event=proposal.event
            or (
                TimePoint.model_validate(current["record"]["event_time"])
                if current["record"].get("event_time")
                else None
            ),
            observation=TimePoint.model_validate(current["record"]["observed_at"]),
            formation=ctx.clock,
            recorded=ctx.clock,
            validity=proposal.validity,
        )
        if self.projection is not None:
            await self.projection.policy_in(tx, "disabled")
        if anchor is not None:
            current_anchor = await self.cognition.get_in(tx, ctx, anchor.reference)
            records = (
                current_anchor,
                *await self.cognition.adopt_semantic_proposal_in(
                    tx, ctx, current_anchor, proposal, times, catalog
                ),
            )
        else:
            records = await self.cognition.adopt_text_formation_in(
                tx,
                ctx,
                text,
                ImportRecord(
                    external_id="observation:" + command.observation_id,
                    text=text,
                    origin=current["record"]["origin"],
                    source_type=current["record"]["source_type"],
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
        scope=ctx.require_subject(),
        payload=command,
    )
    if receipt is not None:
        result = ObservationFormation.model_validate(receipt.result)
        async with self.authority.read(ctx) as view:
            for reference in result.cognition:
                await self.cognition.get_in(view, ctx, reference)
        return result
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
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            return ObservationFormation.model_validate(tx.replay_result)
        for reference in result.cognition:
            await self.cognition.get_in(tx, ctx, reference)
        return tx.result(result)
