"""Evolution operations share the cognitive authority transaction."""

from __future__ import annotations

from typing import TYPE_CHECKING


from nous.platform.persistence import WriteTx

from .evolution_validation import validate_merge, validate_split
from .reference_changes import replace_references_in

if TYPE_CHECKING:
    from .records import CognitiveRecords
from nous_sdk.contracts import (
    AdoptCognition,
    CognitiveRecord,
    ConceptBody,
    EpisodeBody,
    EvidenceLink,
    MergeConcepts,
    ResegmentEpisodes,
    SplitConcept,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError


class EvolutionService:
    def __init__(self, records: CognitiveRecords):
        self.records = records

    async def merge_concepts(
        self, ctx: CallContext, command: MergeConcepts
    ) -> tuple[CognitiveRecord, ...]:
        async with self.records.authority.write(ctx) as tx:
            return await self.merge_concepts_in(tx, ctx, command)

    async def merge_concepts_in(
        self, tx: WriteTx, ctx: CallContext, command: MergeConcepts
    ) -> tuple[CognitiveRecord, ...]:
        records = [
            await self.records.get_in(tx, ctx, ref) for ref in (command.target, *command.sources)
        ]
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
            if ref.id not in {item.reference.id for item in records}
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
        merged = await self.records.adopt_in(
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
                await self.records.adopt_in(
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
        result.extend(
            await replace_references_in(
                self.records,
                tx,
                ctx,
                {record.reference.id: merged.reference for record in records},
                command.reason,
                exclude=frozenset(record.reference.id for record in records),
            )
        )
        return tuple(result)

    async def split_concept(
        self, ctx: CallContext, command: SplitConcept
    ) -> tuple[CognitiveRecord, ...]:
        async with self.records.authority.write(ctx) as tx:
            return await self.split_concept_in(tx, ctx, command)

    async def split_concept_in(
        self, tx: WriteTx, ctx: CallContext, command: SplitConcept
    ) -> tuple[CognitiveRecord, ...]:
        source = await self.records.get_in(tx, ctx, command.source)
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
                await self.records.adopt_in(
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
        replacements = tuple(record.reference for record in result)
        result.append(
            await self.records.adopt_in(
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
        result.extend(
            await replace_references_in(
                self.records,
                tx,
                ctx,
                {source.reference.id: replacements[0]},
                command.reason,
                exclude=frozenset({source.reference.id, *(ref.id for ref in replacements)}),
                partitions=replacements,
                assignments=command.consumers,
                member_partitions={
                    member.id: index
                    for index, part in enumerate(command.partitions)
                    for member in part.members
                },
            )
        )
        return tuple(result)

    async def resegment_episodes(
        self, ctx: CallContext, command: ResegmentEpisodes
    ) -> tuple[CognitiveRecord, ...]:
        async with self.records.authority.write(ctx) as tx:
            return await self.resegment_episodes_in(tx, ctx, command)

    async def resegment_episodes_in(
        self, tx: WriteTx, ctx: CallContext, command: ResegmentEpisodes
    ) -> tuple[CognitiveRecord, ...]:
        originals = [await self.records.get_in(tx, ctx, ref) for ref in command.originals]
        if not all(
            isinstance(record.body, EpisodeBody) and record.body.state == "active"
            for record in originals
        ):
            raise DomainError("episode_resegment", "Resegmentation requires active Episodes")
        tracks = {record.body.track for record in originals if isinstance(record.body, EpisodeBody)}
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
                "episode_members", "Resegmentation must retain each original member exactly once"
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
                await self.records.adopt_in(
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
                await self.records.adopt_in(
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
