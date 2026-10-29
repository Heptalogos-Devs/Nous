"""Authority for exact cognitive revisions and their supporting roots."""

from __future__ import annotations

from collections.abc import Awaitable, Callable
from datetime import datetime
from uuid import uuid4

from nous_wave.contracts import (
    AdoptCognition,
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    CognitiveTimes,
    DomainError,
    EntityBody,
    MediaRecord,
    MediaReference,
    MemoryBody,
    MemoryLifecycle,
    RevisionCause,
    TimePoint,
)
from sqlalchemy import select, update

from nous.identity.service import require_subject_in
from nous.material.coverage import merge_source_regions
from nous.material.derived import DerivedService
from nous.material.service import (
    MaterialService,
    observations,
    require_observation_in,
    source_permitted,
)
from nous.platform.persistence import Authority, ReadView, WriteTx
from nous.references import allocate_in

from . import (
    access,
    evolution,
    formation,
    graph_changes,
    graph_state,
    growth,
    growth_validation,
    known_outputs,
    memory,
    retirement,
    self_model,
    social,
)
from .projection import media_references_in, references_in
from .schema import revisions
from .semantics import capability_for, validate_domain_in


class CognitionService:
    growth_snapshot_in = growth.growth_snapshot_in
    apply_growth_in = growth.apply_growth_in
    validate_growth_in = growth_validation.validate_growth_in
    retire_unsupported_in = retirement.retire_unsupported_in
    link_identity = social.link_identity
    resolve_identity = social.resolve_identity
    change_social_relation = social.change_social_relation
    social_profile = social.social_profile
    history = access.history
    replay_record_in = access.replay_record_in
    replay_records_in = access.replay_records_in
    concept_by_alias = access.concept_by_alias
    set_deleted = access.set_deleted
    set_deleted_in = access.set_deleted_in
    graph_snapshot = graph_state.graph_snapshot
    manage_graph = graph_changes.manage_graph
    qualify_known_outputs_in = known_outputs.qualify_known_outputs_in
    adopt_seed = self_model.adopt_seed
    self_profile = self_model.self_profile
    merge_concepts = evolution.merge_concepts
    merge_concepts_in = evolution.merge_concepts_in
    split_concept = evolution.split_concept
    split_concept_in = evolution.split_concept_in
    resegment_episodes = evolution.resegment_episodes
    resegment_episodes_in = evolution.resegment_episodes_in
    assess_motivation = evolution.assess_motivation
    change_memory_in = memory.change_memory_in
    change_memory = memory.change_memory
    change_memories = memory.change_memories
    change_memories_in = memory.change_memories_in
    recycle_bin = memory.recycle_bin
    decay_memories = memory.decay_memories
    adopt_tags_in = formation.adopt_tags_in
    formation_catalog_in = formation.formation_catalog_in
    formation_catalog = formation.formation_catalog
    adopt_text_formation_in = formation.adopt_text_formation_in
    adopt_semantic_proposal_in = formation.adopt_semantic_proposal_in

    def __init__(
        self,
        authority: Authority,
        *,
        materials: MaterialService | None = None,
        derivations: DerivedService | None = None,
        invalidate: Callable[[], Awaitable[None]] | None = None,
        purge_context_in: Callable[
            [WriteTx, CallContext, tuple[CognitiveRef, ...]], Awaitable[None]
        ]
        | None = None,
    ) -> None:
        self.authority = authority
        self.materials = materials or MaterialService(authority)
        self.derivations = derivations or DerivedService(authority, self.materials)
        self.invalidate = invalidate
        self.purge_context_in = purge_context_in

    async def active_roots_in(self, view: ReadView | WriteTx, ctx: CallContext):
        rows = (
            await view.connection.execute(
                select(observations.c.id, observations.c.material_id).where(
                    observations.c.namespace_id == ctx.namespace_id,
                    observations.c.subject_id == ctx.subject_id,
                    observations.c.active.is_(True),
                )
            )
        ).all()
        return {str(row.id): str(row.material_id) for row in rows}

    async def visible_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        record: CognitiveRecord | MediaRecord,
        roots: dict[str, str] | None = None,
    ) -> bool:
        if isinstance(record, CognitiveRecord):
            for reference in media_references_in(record.body):
                if await self.derivations.availability_in(view, ctx, reference) != "available":
                    return False
        roots = roots if roots is not None else await self.active_roots_in(view, ctx)
        live = [oid for oid in record.source_roots if oid in roots]
        support = set(record.support_roots or record.source_roots)
        if not support.intersection(roots):
            return False
        for group in ctx.source_groups():
            if record.reference.exact() in group or record.reference.id in group:
                continue
            scoped = ctx.for_source_group(group)
            media_roots: set[str] = set()
            if isinstance(record, CognitiveRecord):
                for media in media_references_in(record.body):
                    try:
                        if media.kind in {"representation", "derived_region"}:
                            info = await self.derivations.source_in(view, scoped, media)
                            media_roots.update(info.roots)
                        else:
                            provenance = await self.materials.provenance_in(
                                view, scoped, media, derivations=self.derivations
                            )
                            media_roots.update(provenance.roots)
                    except DomainError as error:
                        if error.code not in {
                            "source_unavailable",
                            "region_not_found",
                            "representation_not_found",
                            "derived_region_not_found",
                        }:
                            raise
            if not all(
                oid in media_roots
                or source_permitted(scoped, roots[oid], oid)
                or (
                    bool(record.source_regions.get(oid))
                    and all(
                        region in group
                        or f"source-region:{region}" in group
                        or f"source_region:{region}" in group
                        for region in record.source_regions[oid]
                    )
                )
                for oid in live
            ):
                return False
        return True

    async def get_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        reference: CognitiveRef,
        *,
        check_visibility: bool = True,
    ):
        row = (
            await view.connection.execute(
                select(revisions.c.record).where(
                    revisions.c.id == reference.id,
                    revisions.c.revision == reference.revision,
                    revisions.c.kind == reference.kind,
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.subject_id == ctx.subject_id,
                )
            )
        ).scalar_one_or_none()
        if row is None:
            raise DomainError("cognition_not_found", "Cognitive revision is unavailable")
        record = CognitiveRecord.model_validate(row)
        if check_visibility and not await self.visible_in(view, ctx, record):
            raise DomainError(
                "source_unavailable", "Cognitive sources are unavailable in this scope"
            )
        return record

    async def adopt_in(self, tx: WriteTx, ctx: CallContext, command: AdoptCognition):
        await require_subject_in(tx, ctx, capability_for(command.body))
        object_id = command.id or str(uuid4())
        head = (
            await tx.connection.execute(
                select(revisions.c.revision).where(
                    revisions.c.id == object_id,
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.subject_id == ctx.subject_id,
                    revisions.c.head.is_(True),
                )
            )
        ).scalar_one_or_none()
        if (head or 0) != command.expected_revision:
            raise DomainError(
                "revision_conflict",
                "Cognition was changed since the expected revision",
                expected=command.expected_revision,
                actual=head or 0,
            )
        existing_record: CognitiveRecord | None = None
        if head:
            existing_record = CognitiveRecord.model_validate(
                (
                    await tx.connection.execute(
                        select(revisions.c.record).where(
                            revisions.c.id == object_id, revisions.c.revision == head
                        )
                    )
                ).scalar_one()
            )
            if existing_record.purged:
                raise DomainError(
                    "cognition_purged", "A purged cognitive identity cannot be revived"
                )
            existing_kind = (
                await tx.connection.execute(
                    select(revisions.c.kind).where(
                        revisions.c.id == object_id, revisions.c.revision == head
                    )
                )
            ).scalar_one()
            if existing_kind != command.body.kind:
                raise DomainError(
                    "cognitive_kind_immutable", "A cognitive identity cannot change its domain kind"
                )
        roots: set[str] = set()
        support: set[str] = set()
        requirements: list[tuple[str, tuple[str, ...]]] = []

        def require_root(observation_id: str, region_ids: tuple[str, ...]) -> None:
            roots.add(observation_id)
            requirements.append((observation_id, region_ids))

        body_evidence = (
            tuple(link for attribute in command.body.attributes for link in attribute.evidence)
            + tuple(
                link
                for identity in command.body.platform_identities
                for link in (
                    *identity.evidence,
                    *(link for attribute in identity.attributes for link in attribute.evidence),
                )
            )
            if isinstance(command.body, EntityBody)
            else ()
        )
        for evidence in (*command.evidence, *body_evidence):
            if evidence.observation_id:
                observation = await require_observation_in(
                    tx, ctx, evidence.observation_id, region_id=evidence.region_id
                )
                source_ref = MediaReference(
                    kind="source_region" if evidence.region_id else "material",
                    id=evidence.region_id or observation["material_id"],
                    observation_id=evidence.observation_id,
                )
                provenance = await self.materials.provenance_in(
                    tx, ctx, source_ref, derivations=self.derivations
                )
                for root in provenance.roots:
                    require_root(root, provenance.regions.get(root, ()))
                if evidence.polarity == "support":
                    support.update(provenance.roots)
                if isinstance(command.body, MemoryBody) and provenance.roots != (
                    evidence.observation_id,
                ):
                    command = command.model_copy(
                        update={
                            "body": command.body.model_copy(
                                update={
                                    "media": tuple(dict.fromkeys((*command.body.media, source_ref)))
                                }
                            )
                        }
                    )
            if evidence.reference:
                source = await self.get_in(tx, ctx, evidence.reference)
                if source.purged and evidence.polarity == "support":
                    raise DomainError(
                        "cognition_purged", "A purged revision cannot support new cognitive content"
                    )
                for observation_id in source.source_roots:
                    require_root(observation_id, source.source_regions.get(observation_id, ()))
                if evidence.polarity == "support":
                    support.update(source.support_roots or source.source_roots)
        for media in media_references_in(command.body):
            if media.kind in {"representation", "derived_region"}:
                info = await self.derivations.source_in(tx, ctx, media)
                for observation_id in info.roots:
                    require_root(observation_id, info.regions.get(observation_id, ()))
                support.update(info.roots)
            else:
                provenance = await self.materials.provenance_in(
                    tx, ctx, media, derivations=self.derivations
                )
                for root in provenance.roots:
                    require_root(root, provenance.regions.get(root, ()))
                support.update(provenance.roots)
        for reference in references_in(command.body):
            if reference.id == object_id and reference.revision >= (head or 0) + 1:
                raise DomainError("self_reference", "A revision cannot reference itself")
            source = await self.get_in(tx, ctx, reference)
            for observation_id in source.source_roots:
                require_root(observation_id, source.source_regions.get(observation_id, ()))
            if command.body.kind == "journal" and source.commit_seq > command.body.knowledge_cutoff:
                raise DomainError(
                    "knowledge_cutoff", "Journal cites knowledge after its writing perspective"
                )
        await validate_domain_in(self, tx, ctx, command.body, object_id)
        if not support:
            raise DomainError(
                "grounding_required", "Cognitive adoption requires a live supporting source"
            )
        if command.body.kind == "schema" and not command.body.support:
            raise DomainError("support_required", "Schema requires supporting cognition")
        if command.body.kind == "episode":
            ordinals = [member.ordinal for member in command.body.members]
            if len(ordinals) != len(set(ordinals)):
                raise DomainError("episode_order", "Episode member ordinals must be unique")
        now = ctx.clock
        times = command.times or CognitiveTimes(formation=now, recorded=now)
        # The recorded axis is the authority's real clock, independent of host event time.
        recorded = TimePoint(
            coordinate=int(datetime.fromisoformat(tx.recorded_at).timestamp() * 1000)
        )
        times = times.model_copy(update={"recorded": recorded})
        result = CognitiveRecord(
            reference=CognitiveRef(kind=command.body.kind, id=object_id, revision=(head or 0) + 1),
            namespace_id=ctx.namespace_id,
            subject_id=ctx.require_subject(),
            body=command.body,
            evidence=command.evidence,
            source_roots=tuple(sorted(roots)),
            support_roots=tuple(sorted(support)),
            source_regions=merge_source_regions(requirements),
            times=times,
            producer_call_id=command.producer_call_id,
            commit_seq=tx.commit_seq,
            lifecycle=(
                existing_record.lifecycle
                if existing_record and existing_record.lifecycle
                else MemoryLifecycle(anchor_unix_ms=recorded.coordinate)
            )
            if command.body.kind == "memory"
            else None,
            cause=RevisionCause(
                action="adopt",
                actor=ctx.caller_id,
                reason="cognitive adoption",
                parent=existing_record.reference if existing_record else None,
            ),
        )
        result = result.model_copy(
            update={"addresses": await allocate_in(tx, ctx, result.reference)}
        )
        if head:
            await tx.connection.execute(
                update(revisions)
                .where(revisions.c.id == object_id, revisions.c.head.is_(True))
                .values(head=False)
            )
        await tx.connection.execute(
            revisions.insert().values(
                id=object_id,
                revision=result.reference.revision,
                namespace_id=ctx.namespace_id,
                subject_id=ctx.require_subject(),
                kind=result.reference.kind,
                head=True,
                record=result.model_dump(mode="json"),
                commit_seq=tx.commit_seq,
            )
        )
        return result

    async def adopt(self, ctx: CallContext, command: AdoptCognition):
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="cognition.adopt",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                return await self.replay_record_in(
                    tx, ctx, CognitiveRecord.model_validate(tx.replay_result)
                )
            return tx.result(await self.adopt_in(tx, ctx, command))

    async def get(self, ctx: CallContext, reference: CognitiveRef):
        async with self.authority.read(ctx) as view:
            await require_subject_in(view, ctx)
            return await self.get_in(view, ctx, reference)
