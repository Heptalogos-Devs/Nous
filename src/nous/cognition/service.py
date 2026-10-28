"""Authority for exact cognitive revisions and their supporting roots."""

from __future__ import annotations

from collections.abc import Awaitable, Callable, Iterator
from uuid import uuid4

from nous_wave.contracts import (
    AdoptCognition,
    CallContext,
    CognitiveBody,
    CognitiveRecord,
    CognitiveRef,
    CognitiveTimes,
    ConceptBody,
    ConcernBody,
    DesireBody,
    DesiredCondition,
    DomainError,
    EpisodeBody,
    EvidenceLink,
    MediaRecord,
    MemoryBody,
    MemoryChange,
    MemoryLifecycle,
    MergeConcepts,
    ResegmentEpisodes,
    RevisionCause,
    SplitConcept,
    TimePoint,
)
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import JSON, Boolean, Column, Integer, String, Table, select, update

from nous.identity.service import require_subject_in
from nous.material.service import observations, require_observation_in, source_permitted
from nous.platform.persistence import Authority, ReadView, WriteTx, metadata

revisions = Table(
    "cognitive_revisions",
    metadata,
    Column("id", String, primary_key=True),
    Column("revision", Integer, primary_key=True),
    Column("namespace_id", String, nullable=False, index=True),
    Column("subject_id", String, nullable=False, index=True),
    Column("kind", String, nullable=False, index=True),
    Column("head", Boolean, nullable=False),
    Column("record", JSON, nullable=False),
    Column("commit_seq", Integer, nullable=False),
)
vectors = Table(
    "cognitive_vectors",
    metadata,
    Column("reference", String, primary_key=True),
    Column("space", String, primary_key=True),
    Column("model", String, nullable=False),
    Column("dimensions", Integer, nullable=False),
    Column("vector", JSON, nullable=False),
    Column("call_id", String),
    Column("commit_seq", Integer, nullable=False),
)


def body_text(body: CognitiveBody | None) -> str:
    if body is None:
        return ""
    value = body.model_dump(mode="json")
    return "\n".join(
        str(value[key])
        for key in (
            "text",
            "summary",
            "label",
            "description",
            "title",
            "narrative",
            "claim",
            "understanding",
            "expression",
            "meaning",
            "relation",
            "facet",
        )
        if value.get(key)
    )


def references_in(
    value: JsonValue | list[JsonValue] | tuple[JsonValue, ...],
) -> Iterator[CognitiveRef]:
    if isinstance(value, dict):
        if {"kind", "id", "revision"}.issubset(value):
            yield CognitiveRef.model_validate(value)
        else:
            for child in value.values():
                yield from references_in(child)
    elif isinstance(value, (list, tuple)):
        for child in value:
            yield from references_in(child)


class CognitionService:
    def __init__(self, authority: Authority, *, invalidate: Callable[[], Awaitable[None]] | None = None, purge_context_in: Callable[[WriteTx, CallContext, tuple[CognitiveRef, ...]], Awaitable[None]] | None = None) -> None:
        self.authority = authority
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
        roots = roots if roots is not None else await self.active_roots_in(view, ctx)
        live = [oid for oid in record.source_roots if oid in roots]
        support = set(record.support_roots or record.source_roots)
        if not support.intersection(roots):
            return False
        if ctx.allowed_source_ids is not None and (
            record.reference.exact() in ctx.allowed_source_ids
            or record.reference.id in ctx.allowed_source_ids
        ):
            return True
        return bool(support.intersection(roots)) and all(
            source_permitted(ctx, roots[oid], oid)
            or (
                bool(record.source_regions.get(oid))
                and ctx.allowed_source_ids is not None
                and all(region in ctx.allowed_source_ids for region in record.source_regions[oid])
            )
            for oid in live
        )

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
        capability = (
            "memory"
            if command.body.kind
            in {
                "memory",
                "memory_semantics",
                "concept",
                "entity",
                "episode",
                "journal",
                "schema",
                "association",
            }
            else "social"
            if command.body.kind in {"social_relation", "language_convention"}
            else "motivation"
            if command.body.kind in {"desire", "concern"}
            else "self"
        )
        await require_subject_in(tx, ctx, capability)
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
            existing_record = CognitiveRecord.model_validate((await tx.connection.execute(select(revisions.c.record).where(revisions.c.id == object_id, revisions.c.revision == head))).scalar_one())
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
        requirements: dict[str, set[str] | None] = {}

        def require_root(observation_id: str, region_ids: tuple[str, ...]) -> None:
            roots.add(observation_id)
            if not region_ids:
                requirements[observation_id] = None
            elif observation_id not in requirements:
                requirements[observation_id] = set(region_ids)
            elif requirements[observation_id] is not None:
                existing = requirements[observation_id]
                if existing is not None:
                    existing.update(region_ids)

        for evidence in command.evidence:
            if evidence.observation_id:
                await require_observation_in(
                    tx, ctx, evidence.observation_id, region_id=evidence.region_id
                )
                require_root(
                    evidence.observation_id, (evidence.region_id,) if evidence.region_id else ()
                )
                if evidence.polarity == "support":
                    support.add(evidence.observation_id)
            if evidence.reference:
                source = await self.get_in(tx, ctx, evidence.reference)
                for observation_id in source.source_roots:
                    require_root(observation_id, source.source_regions.get(observation_id, ()))
                if evidence.polarity == "support":
                    support.update(source.support_roots or source.source_roots)
        for reference in references_in(command.body.model_dump(mode="json")):
            if reference.id == object_id and reference.revision >= (head or 0) + 1:
                raise DomainError("self_reference", "A revision cannot reference itself")
            source = await self.get_in(tx, ctx, reference)
            if command.body.kind == "journal" and source.commit_seq > command.body.knowledge_cutoff:
                raise DomainError(
                    "knowledge_cutoff", "Journal cites knowledge after its writing perspective"
                )
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
        recorded = TimePoint(coordinate=int(__import__("time").time() * 1000))
        times = times.model_copy(update={"recorded": recorded})
        result = CognitiveRecord(
            reference=CognitiveRef(kind=command.body.kind, id=object_id, revision=(head or 0) + 1),
            namespace_id=ctx.namespace_id,
            subject_id=ctx.require_subject(),
            body=command.body,
            evidence=command.evidence,
            source_roots=tuple(sorted(roots)),
            support_roots=tuple(sorted(support)),
            source_regions={
                oid: tuple(sorted(regions)) for oid, regions in requirements.items() if regions
            },
            times=times,
            producer_call_id=command.producer_call_id,
            commit_seq=tx.commit_seq,
            lifecycle=(existing_record.lifecycle if existing_record and existing_record.lifecycle else MemoryLifecycle(anchor_unix_ms=recorded.coordinate)) if command.body.kind == "memory" else None,
            cause=RevisionCause(action="adopt", actor=ctx.caller_id, reason="cognitive adoption", parent=existing_record.reference if existing_record else None),
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
                return CognitiveRecord.model_validate(tx.replay_result)
            return tx.result(await self.adopt_in(tx, ctx, command))

    async def get(self, ctx: CallContext, reference: CognitiveRef):
        async with self.authority.read(ctx) as view:
            await require_subject_in(view, ctx)
            return await self.get_in(view, ctx, reference)

    async def merge_concepts(
        self, ctx: CallContext, command: MergeConcepts
    ) -> tuple[CognitiveRecord, ...]:
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="concept.merge",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(tx.replay_result)
            records = [
                await self.get_in(tx, ctx, ref) for ref in (command.target, *command.sources)
            ]
            if len({record.reference.id for record in records}) != len(records) or not all(
                isinstance(record.body, ConceptBody) and record.body.state == "active"
                for record in records
            ):
                raise DomainError("concept_merge", "Merge requires distinct active Concepts")
            target = records[0]
            assert isinstance(target.body, ConceptBody)
            aliases = tuple(
                dict.fromkeys(
                    alias
                    for record in records
                    if isinstance(record.body, ConceptBody)
                    for alias in (record.body.label, *record.body.aliases)
                    if alias.casefold() != target.body.label.casefold()
                )
            )
            members = {
                ref.exact(): ref
                for record in records
                if isinstance(record.body, ConceptBody)
                for ref in record.body.members
            }
            body = target.body.model_copy(
                update={
                    "aliases": aliases,
                    "members": tuple(members.values()),
                    "lineage": tuple(record.reference for record in records),
                }
            )
            evidence = tuple(
                EvidenceLink(reference=record.reference, role=command.reason) for record in records
            )
            merged = await self.adopt_in(
                tx,
                ctx,
                AdoptCognition(
                    id=target.reference.id,
                    expected_revision=target.reference.revision,
                    body=body,
                    evidence=evidence,
                    times=target.times,
                ),
            )
            result = [merged]
            for source in records[1:]:
                assert isinstance(source.body, ConceptBody)
                retired = source.body.model_copy(
                    update={
                        "canonical": merged.reference,
                        "state": "merged",
                        "successors": (merged.reference,),
                        "lineage": (source.reference,),
                    }
                )
                result.append(
                    await self.adopt_in(
                        tx,
                        ctx,
                        AdoptCognition(
                            id=source.reference.id,
                            expected_revision=source.reference.revision,
                            body=retired,
                            evidence=(
                                EvidenceLink(reference=source.reference, role=command.reason),
                            ),
                            times=source.times,
                        ),
                    )
                )
            return tx.result(tuple(result))

    async def split_concept(
        self, ctx: CallContext, command: SplitConcept
    ) -> tuple[CognitiveRecord, ...]:
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="concept.split",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(tx.replay_result)
            source = await self.get_in(tx, ctx, command.source)
            if not isinstance(source.body, ConceptBody) or source.body.state != "active":
                raise DomainError("concept_split", "Split requires an active Concept")
            original = {ref.exact() for ref in source.body.members}
            members = [ref.exact() for partition in command.partitions for ref in partition.members]
            if set(members) != original or len(members) != len(set(members)):
                raise DomainError(
                    "concept_partition",
                    "Split partitions must preserve every original member exactly once",
                )
            result: list[CognitiveRecord] = []
            for partition in command.partitions:
                body = ConceptBody(
                    label=partition.label,
                    description=source.body.description,
                    members=partition.members,
                    lineage=(source.reference,),
                )
                result.append(
                    await self.adopt_in(
                        tx,
                        ctx,
                        AdoptCognition(
                            body=body,
                            evidence=(
                                EvidenceLink(reference=source.reference, role=command.reason),
                            ),
                            times=source.times,
                        ),
                    )
                )
            retired = source.body.model_copy(
                update={
                    "state": "split",
                    "successors": tuple(record.reference for record in result),
                    "lineage": (source.reference,),
                }
            )
            result.append(
                await self.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        id=source.reference.id,
                        expected_revision=source.reference.revision,
                        body=retired,
                        evidence=(EvidenceLink(reference=source.reference, role=command.reason),),
                        times=source.times,
                    ),
                )
            )
            return tx.result(tuple(result))

    async def resegment_episodes(
        self, ctx: CallContext, command: ResegmentEpisodes
    ) -> tuple[CognitiveRecord, ...]:
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="episode.resegment",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(tx.replay_result)
            originals = [await self.get_in(tx, ctx, ref) for ref in command.originals]
            if not all(
                isinstance(record.body, EpisodeBody) and record.body.state == "active"
                for record in originals
            ):
                raise DomainError("episode_resegment", "Resegmentation requires active Episodes")
            tracks = {
                record.body.track for record in originals if isinstance(record.body, EpisodeBody)
            }
            if len(tracks) != 1 or any(body.track not in tracks for body in command.replacements):
                raise DomainError(
                    "episode_track", "Local resegmentation must remain in one episode track"
                )
            old_members = {
                member.reference.exact()
                for record in originals
                if isinstance(record.body, EpisodeBody)
                for member in record.body.members
            }
            new_members = [
                member.reference.exact() for body in command.replacements for member in body.members
            ]
            if set(new_members) != old_members or len(new_members) != len(set(new_members)):
                raise DomainError(
                    "episode_members",
                    "Resegmentation must retain each original member exactly once",
                )
            evidence = tuple(
                EvidenceLink(reference=record.reference, role=command.reason)
                for record in originals
            )
            result: list[CognitiveRecord] = []
            for body in command.replacements:
                replacement = body.model_copy(
                    update={"lineage": tuple(record.reference for record in originals)}
                )
                result.append(
                    await self.adopt_in(
                        tx, ctx, AdoptCognition(body=replacement, evidence=evidence)
                    )
                )
            successors = tuple(record.reference for record in result)
            for source in originals:
                assert isinstance(source.body, EpisodeBody)
                retired = source.body.model_copy(
                    update={"state": "resegmented", "successors": successors}
                )
                result.append(
                    await self.adopt_in(
                        tx,
                        ctx,
                        AdoptCognition(
                            id=source.reference.id,
                            expected_revision=source.reference.revision,
                            body=retired,
                            evidence=(
                                EvidenceLink(reference=source.reference, role=command.reason),
                            ),
                            times=source.times,
                        ),
                    )
                )
            return tx.result(tuple(result))

    async def assess_motivation(
        self, ctx: CallContext, reference: CognitiveRef
    ) -> dict[str, JsonValue]:
        record = await self.get(ctx, reference)
        if not isinstance(record.body, (DesireBody, ConcernBody)):
            raise DomainError("motivation_required", "Assessment requires Desire or Concern")

        def evaluate(condition: DesiredCondition) -> str:
            if condition.hold is not None and not condition.hold.contains(ctx.clock):
                return "unknown"
            if condition.operator == "predicate":
                return condition.state
            children = [evaluate(child) for child in condition.children]
            if condition.operator == "not":
                return {"true": "false", "false": "true", "unknown": "unknown"}[children[0]]
            if condition.operator == "all":
                return (
                    "false"
                    if "false" in children
                    else "unknown"
                    if "unknown" in children
                    else "true"
                )
            return "true" if "true" in children else "unknown" if "unknown" in children else "false"

        state = evaluate(record.body.condition)
        mode = record.body.condition.mode
        satisfaction = (
            "unknown"
            if state == "unknown"
            else ("true" if (state == "true") == (mode in {"achieve", "maintain"}) else "false")
        )
        return {
            "reference": reference.model_dump(mode="json"),
            "state": state,
            "satisfaction": satisfaction,
            "mode": mode,
        }

    async def _change_memory_in(self, tx: WriteTx, ctx: CallContext, command: MemoryChange) -> CognitiveRecord:
        from .lifecycle import transition
        previous = await self.get_in(tx, ctx, command.reference)
        if previous.reference.kind != "memory" or previous.purged or not isinstance(previous.body, MemoryBody):
            raise DomainError("memory_unavailable", "Maintenance requires a nonpurged Memory revision")
        if previous.deleted and command.action not in {"restore", "purge"}:
            raise DomainError("memory_deleted", "Restore the memory before changing it")
        if command.body is not None and command.action != "revise":
            raise DomainError("invalid_memory_edit", "A body edit is accepted only for revise")
        if command.action == "revise" and command.body is None:
            raise DomainError("invalid_memory_edit", "Revise requires the complete edited memory body")
        now = int(__import__("datetime").datetime.fromisoformat(tx.recorded_at).timestamp() * 1000)
        lifecycle = previous.lifecycle or MemoryLifecycle(anchor_unix_ms=previous.times.recorded.coordinate)
        if command.action in {"trash", "purge"} and lifecycle.protected(now) and not command.force:
            raise DomainError("memory_protected", "Memory is protected; unprotect or explicitly force the change")
        result = await self.adopt_in(tx, ctx, AdoptCognition(id=previous.reference.id,
            expected_revision=command.reference.revision, body=command.body or previous.body,
            evidence=(EvidenceLink(reference=previous.reference, role=command.reason or command.action),),
            times=previous.times, producer_call_id=previous.producer_call_id))
        result = result.model_copy(update={"lifecycle": transition(lifecycle, command, now),
            "deleted": command.action in {"trash", "purge"} if command.action in {"trash", "purge", "restore"} else previous.deleted,
            "cause": RevisionCause(action=command.action, actor=ctx.caller_id, reason=command.reason, parent=previous.reference)})
        if command.action == "purge":
            result = result.model_copy(update={"body": None, "purged": True, "deleted": True})
            history = [CognitiveRecord.model_validate(value) for value in (await tx.connection.execute(select(revisions.c.record).where(revisions.c.id == previous.reference.id, revisions.c.namespace_id == ctx.namespace_id, revisions.c.subject_id == ctx.subject_id))).scalars()]
            for historical in history:
                tombstone = historical.model_copy(update={"body": None, "purged": True, "deleted": True})
                await tx.connection.execute(update(revisions).where(revisions.c.id == historical.reference.id, revisions.c.revision == historical.reference.revision).values(record=tombstone.model_dump(mode="json")))
            references = tuple(record.reference for record in history)
            await tx.connection.execute(vectors.delete().where(vectors.c.reference.in_([ref.exact() for ref in references])))
            if self.purge_context_in:
                await self.purge_context_in(tx, ctx, references)
            def contains_body(value: JsonValue) -> bool:
                if isinstance(value, list):
                    return any(contains_body(child) for child in value)
                if not isinstance(value, dict):
                    return False
                reference = value.get("reference")
                return (isinstance(reference, dict) and reference.get("id") == previous.reference.id and "body" in value) or any(contains_body(child) for child in value.values())
            await self.authority.redact_receipts_in(tx, contains_body)
        elif body_text(result.body) == body_text(previous.body):
            assets = (await tx.connection.execute(select(vectors).where(vectors.c.reference == previous.reference.exact()))).mappings().all()
            for asset in assets:
                await tx.connection.execute(vectors.insert().values(**{**asset, "reference": result.reference.exact(), "commit_seq": tx.commit_seq}))
        await tx.connection.execute(update(revisions).where(revisions.c.id == result.reference.id, revisions.c.revision == result.reference.revision).values(record=result.model_dump(mode="json")))
        return result

    async def change_memory(self, ctx: CallContext, command: MemoryChange) -> CognitiveRecord:
        async with self.authority.write(ctx, command.operation_id, operation_kind="memory.change", scope=ctx.require_subject(), payload=command) as tx:
            if tx.replayed:
                return CognitiveRecord.model_validate(tx.replay_result)
            result = tx.result(await self._change_memory_in(tx, ctx, command))
        if command.action == "purge" and self.invalidate:
            await self.invalidate()
        return result

    async def recycle_bin(self, ctx: CallContext, limit: int = 50) -> tuple[CognitiveRecord, ...]:
        if not 1 <= limit <= 1000:
            raise DomainError("invalid_limit", "Recycle-bin limit must be between 1 and 1000")
        async with self.authority.read(ctx) as view:
            await require_subject_in(view, ctx, "memory")
            result: list[CognitiveRecord] = []
            roots = await self.active_roots_in(view, ctx)
            for value in (await view.connection.execute(select(revisions.c.record).where(revisions.c.namespace_id == ctx.namespace_id, revisions.c.subject_id == ctx.subject_id, revisions.c.kind == "memory", revisions.c.head.is_(True)).order_by(revisions.c.commit_seq.desc()))).scalars():
                record = CognitiveRecord.model_validate(value)
                if record.deleted and await self.visible_in(view, ctx, record, roots):
                    result.append(record)
                if len(result) >= limit:
                    break
            return tuple(result)

    async def decay_memories(self, ctx: CallContext, operation_id: str, limit: int = 1000):
        if not 1 <= limit <= 1000:
            raise DomainError("invalid_limit", "Decay limit must be between 1 and 1000")
        from .lifecycle import transition
        async with self.authority.write(ctx, operation_id, operation_kind="memory.decay", scope=ctx.require_subject(), payload={"limit": limit}) as tx:
            if tx.replayed:
                return tx.replay_result
            await require_subject_in(tx, ctx, "memory")
            now = int(__import__("datetime").datetime.fromisoformat(tx.recorded_at).timestamp() * 1000)
            rows = (await tx.connection.execute(select(revisions.c.record).where(revisions.c.namespace_id == ctx.namespace_id, revisions.c.subject_id == ctx.subject_id, revisions.c.kind == "memory", revisions.c.head.is_(True)).order_by(revisions.c.commit_seq).limit(limit))).scalars().all()
            changed: list[str] = []
            for value in rows:
                record = CognitiveRecord.model_validate(value)
                if record.deleted or not await self.visible_in(tx, ctx, record):
                    continue
                command = MemoryChange(reference=record.reference, action="decay", reason="scheduled memory retention evaluation")
                lifecycle = record.lifecycle or MemoryLifecycle(anchor_unix_ms=record.times.recorded.coordinate)
                if transition(lifecycle, command, now) != lifecycle:
                    changed.append((await self._change_memory_in(tx, ctx, command)).reference.exact())
            return tx.result({"scanned": len(rows), "changed": changed, "complete": len(rows) < limit})
