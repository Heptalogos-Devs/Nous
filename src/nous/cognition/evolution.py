"""Evolution operations share the cognitive authority transaction."""

from __future__ import annotations

from typing import TYPE_CHECKING

from pydantic import JsonValue, TypeAdapter

from nous.platform.persistence import WriteTx

from .evolution_validation import validate_merge, validate_split

if TYPE_CHECKING:
    from .service import CognitionService

from nous_sdk.contracts import (
    AdoptCognition,
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    ConceptBody,
    ConcernBody,
    DesireBody,
    DesiredCondition,
    DomainError,
    EpisodeBody,
    EvidenceLink,
    MergeConcepts,
    ResegmentEpisodes,
    SplitConcept,
)


async def merge_concepts(
    self: CognitionService, ctx: CallContext, command: MergeConcepts
) -> tuple[CognitiveRecord, ...]:
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="concept.merge",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            return await self.replay_records_in(
                tx, ctx, TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(tx.replay_result)
            )
        return tx.result(await self.merge_concepts_in(tx, ctx, command))


async def merge_concepts_in(
    self: CognitionService, tx: WriteTx, ctx: CallContext, command: MergeConcepts
) -> tuple[CognitiveRecord, ...]:
    records = [await self.get_in(tx, ctx, ref) for ref in (command.target, *command.sources)]
    validate_merge(records)
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
                    evidence=(EvidenceLink(reference=source.reference, role=command.reason),),
                    times=source.times,
                ),
            )
        )
    return tuple(result)


async def split_concept(
    self: CognitionService, ctx: CallContext, command: SplitConcept
) -> tuple[CognitiveRecord, ...]:
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="concept.split",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            return await self.replay_records_in(
                tx, ctx, TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(tx.replay_result)
            )
        return tx.result(await self.split_concept_in(tx, ctx, command))


async def split_concept_in(
    self: CognitionService, tx: WriteTx, ctx: CallContext, command: SplitConcept
) -> tuple[CognitiveRecord, ...]:
    source = await self.get_in(tx, ctx, command.source)
    validate_split(source, command.partitions)
    assert isinstance(source.body, ConceptBody)
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
                    evidence=(EvidenceLink(reference=source.reference, role=command.reason),),
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
    return tuple(result)


async def resegment_episodes(
    self: CognitionService, ctx: CallContext, command: ResegmentEpisodes
) -> tuple[CognitiveRecord, ...]:
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="episode.resegment",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            return await self.replay_records_in(
                tx, ctx, TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(tx.replay_result)
            )
        return tx.result(await self.resegment_episodes_in(tx, ctx, command))


async def resegment_episodes_in(
    self: CognitionService, tx: WriteTx, ctx: CallContext, command: ResegmentEpisodes
) -> tuple[CognitiveRecord, ...]:
    originals = [await self.get_in(tx, ctx, ref) for ref in command.originals]
    if not all(
        isinstance(record.body, EpisodeBody) and record.body.state == "active"
        for record in originals
    ):
        raise DomainError("episode_resegment", "Resegmentation requires active Episodes")
    tracks = {record.body.track for record in originals if isinstance(record.body, EpisodeBody)}
    if len(tracks) != 1 or any(body.track not in tracks for body in command.replacements):
        raise DomainError("episode_track", "Local resegmentation must remain in one episode track")
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
        EvidenceLink(reference=record.reference, role=command.reason) for record in originals
    )
    result: list[CognitiveRecord] = []
    for body in command.replacements:
        replacement = body.model_copy(
            update={"lineage": tuple(record.reference for record in originals)}
        )
        result.append(
            await self.adopt_in(tx, ctx, AdoptCognition(body=replacement, evidence=evidence))
        )
    successors = tuple(record.reference for record in result)
    for source in originals:
        assert isinstance(source.body, EpisodeBody)
        retired = source.body.model_copy(update={"state": "resegmented", "successors": successors})
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
    return tuple(result)


async def assess_motivation(
    self: CognitionService, ctx: CallContext, reference: CognitiveRef
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
                "false" if "false" in children else "unknown" if "unknown" in children else "true"
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
