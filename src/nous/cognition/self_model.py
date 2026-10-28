"""Multi-document Self seeds preserve actual source contacts and explicit revisions."""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING

from nous_wave.contracts import (
    AdoptCognition,
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    CognitiveTimes,
    DomainError,
    EpistemicStance,
    EvidenceLink,
    MemoryBody,
    SelfFacetBody,
    TimePoint,
)
from nous_wave.contracts.self_model import AdoptSeed, SeedResult
from sqlalchemy import select

from nous.identity.service import require_subject_in
from nous.material.service import MaterialRecord
from nous.platform.content import ContentLease, ContentObject

from .schema import revisions

if TYPE_CHECKING:
    from .service import CognitionService


async def adopt_seed(self: CognitionService, ctx: CallContext, command: AdoptSeed) -> SeedResult:
    receipt = await self.authority.receipt(
        ctx,
        command.operation_id,
        operation_kind="self.seed",
        scope=ctx.require_subject(),
        payload=command,
    )
    if receipt is not None:
        return SeedResult.model_validate(receipt.result)
    async with self.authority.read(ctx) as view:
        await require_subject_in(view, ctx, "self")
    prepared: list[tuple[MaterialRecord | None, str, ContentLease | None]] = []
    try:
        for entry in command.documents:
            document = entry.document
            material = (
                await self.materials.read(ctx, document.material_id)
                if document.material_id
                else None
            )
            text = material["text"] if material else document.inline_text()
            if text is None:
                if self.materials.content_store is None:
                    raise DomainError(
                        "content_unavailable", "The Seed original bytes are unavailable"
                    )
                assert material is not None
                async with self.materials.content_store.protect(
                    ContentObject(
                        material["content_hash"], material["byte_len"], material["media_type"]
                    )
                ) as path:
                    text = (await asyncio.to_thread(path.read_bytes)).decode("utf-8-sig")
            lease = (
                await self.materials.content_store.publish_bytes(text.encode(), document.media_type)
                if self.materials.content_store and material is None
                else None
            )
            prepared.append((material, text, lease))
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="self.seed",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                return SeedResult.model_validate(tx.replay_result)
            await require_subject_in(tx, ctx, "self")
            memories: list[CognitiveRef] = []
            facets: list[CognitiveRef] = []
            observation_ids: list[str] = []
            for entry, (original, text, lease) in zip(command.documents, prepared, strict=True):
                document = entry.document.model_copy(update={"source_type": "self_seed"})
                material = (
                    await self.materials.read_in(tx, ctx, original["id"])
                    if original
                    else await self.materials.put_in(tx, ctx, document, lease)
                )
                event = await self.materials.observe_in(
                    tx, ctx, material["id"], document, kind="contact"
                )
                observation_ids.append(event["id"])
                evidence = (EvidenceLink(observation_id=event["id"]),)
                times = CognitiveTimes(
                    event=document.event_time,
                    observation=TimePoint.model_validate(event["observed_at"]),
                    formation=ctx.clock,
                    recorded=ctx.clock,
                )
                stance = EpistemicStance(
                    classification="simulated", attribution="self_seed:" + material["id"]
                )
                if entry.enter_memory:
                    memory = await self.adopt_in(
                        tx,
                        ctx,
                        AdoptCognition(
                            body=MemoryBody(
                                text=text, role="procedural_experience", epistemic=stance
                            ),
                            evidence=evidence,
                            times=times,
                        ),
                    )
                    memories.append(memory.reference)
                for facet in entry.facets:
                    prior = await self.get_in(tx, ctx, facet.target) if facet.target else None
                    if prior is not None and not isinstance(prior.body, SelfFacetBody):
                        raise DomainError(
                            "seed_target_invalid",
                            "A seeded facet can only revise an existing SelfFacet",
                        )
                    record = await self.adopt_in(
                        tx,
                        ctx,
                        AdoptCognition(
                            id=prior.reference.id if prior else None,
                            expected_revision=prior.reference.revision if prior else 0,
                            body=SelfFacetBody(
                                facet=facet.facet,
                                description=facet.description,
                                contexts=facet.contexts,
                                exceptions=facet.exceptions,
                                epistemic=stance,
                            ),
                            evidence=(
                                *evidence,
                                *(
                                    (
                                        EvidenceLink(
                                            reference=prior.reference,
                                            polarity="boundary",
                                            role="superseded_seed",
                                        ),
                                    )
                                    if prior
                                    else ()
                                ),
                            ),
                            times=times,
                        ),
                    )
                    facets.append(record.reference)
            result = SeedResult(
                observation_ids=tuple(observation_ids),
                memories=tuple(memories),
                facets=tuple(facets),
                commit_seq=tx.commit_seq,
            )
            return tx.result(result)
    finally:
        if self.materials.content_store:
            for _, _, lease in prepared:
                if lease is not None:
                    await self.materials.content_store.release(lease)


async def self_profile(self: CognitionService, ctx: CallContext) -> tuple[CognitiveRecord, ...]:
    async with self.authority.read(ctx) as view:
        await require_subject_in(view, ctx, "self")
        rows = (
            await view.connection.execute(
                select(revisions.c.record).where(
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.subject_id == ctx.subject_id,
                    revisions.c.head.is_(True),
                    revisions.c.kind.in_(("self_facet", "narrative_thread", "domain_knowledge")),
                )
            )
        ).scalars()
        result: list[CognitiveRecord] = []
        for value in rows:
            record = CognitiveRecord.model_validate(value)
            if not record.deleted and await self.visible_in(view, ctx, record):
                result.append(record)
        return tuple(result)
