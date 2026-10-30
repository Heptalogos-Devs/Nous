"""Complete-source plans use DBOS execution and canonical Episode revision CAS."""

from __future__ import annotations

from collections import Counter
from typing import TYPE_CHECKING

from nous_wave.contracts import (
    AdoptCognition,
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    DomainError,
    EpisodeBody,
    EpisodeMember,
    EvidenceLink,
    RevisionCause,
    TimeRange,
)
from nous_wave.contracts.episodes import (
    EpisodeRebuildIntent,
    EpisodeRebuildSubmission,
    EpisodeSegmentationDraft,
    EpisodeSegmentationInput,
    EpisodeSegmentationMember,
    EpisodeSourceSnapshot,
    RebuildEpisodes,
)
from pydantic import JsonValue, TypeAdapter

from nous.identity.service import require_subject_in
from nous.platform.execution import JobSpec

from .projection import body_text
from .revision_state import RevisionState

if TYPE_CHECKING:
    from .episodes import EpisodeService


async def submit_rebuild(
    self: EpisodeService, ctx: CallContext, command: RebuildEpisodes
) -> EpisodeRebuildSubmission:
    if len(set(command.sources)) != len(command.sources):
        raise DomainError("episode_source_duplicate", "Source names must be distinct")
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="episode.rebuild",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            result = EpisodeRebuildSubmission.model_validate(tx.replay_result)
            for source in result.sources:
                await self.source_snapshot_in(tx, ctx, source)
            return result
        await require_subject_in(tx, ctx, "memory")
        snapshots: list[EpisodeSourceSnapshot] = []
        for source in command.sources:
            members, originals, observation_ids = await self.source_snapshot_in(tx, ctx, source)
            snapshots.append(
                EpisodeSourceSnapshot(
                    source=source,
                    observations=observation_ids,
                    members=tuple(record.reference for record in members),
                    originals=tuple(record.reference for record in originals),
                )
            )
        intent = EpisodeRebuildIntent(context=ctx, command=command, snapshots=tuple(snapshots))
        job = await tx.enqueue(
            JobSpec(
                "nous.episodes.rebuild",
                "formation",
                args=(intent.model_dump(mode="json"),),
            )
        )
        return tx.result(EpisodeRebuildSubmission(job_id=job.workflow_id, sources=command.sources))


def groups(
    records: tuple[CognitiveRecord, ...], command: RebuildEpisodes
) -> list[list[CognitiveRecord]]:
    """Adapt A-Memorix's interval-component ordering before model-call size splitting.

    Native occurrence points and clock domains are retained. Formation order only
    places unknown-time inputs in a batch; it never supplies an occurrence time.
    """

    def order(record: CognitiveRecord) -> tuple[str, int, str]:
        point = record.times.event
        return (
            point.domain if point else "",
            point.coordinate if point else record.times.formation.coordinate,
            record.reference.exact(),
        )

    result: list[list[CognitiveRecord]] = []
    current: list[CognitiveRecord] = []
    size = 0
    previous: tuple[str, int, str] | None = None
    for record in sorted(records, key=order):
        point = order(record)
        length = len(body_text(record.body))
        boundary = previous is not None and (
            point[0] != previous[0]
            or bool(point[0])
            and point[1] > previous[1] + command.group_gap_ms
        )
        if current and (
            boundary
            or len(current) >= command.max_members_per_call
            or size + length > command.max_characters_per_call
        ):
            result.append(current)
            current, size = [], 0
        current.append(record)
        size += length
        previous = point
    if current:
        result.append(current)
    return result


async def run_rebuild(self: EpisodeService, payload: JsonValue) -> JsonValue:
    intent = EpisodeRebuildIntent.model_validate(payload)
    ctx, command = intent.context, intent.command
    operation = command.operation_id + ":publish"
    receipt = await self.authority.receipt(
        ctx,
        operation,
        operation_kind="episode.publish",
        scope=ctx.require_subject(),
        payload=intent,
    )
    if receipt is not None:
        return receipt.result
    plans: list[tuple[EpisodeSourceSnapshot, list[tuple[EpisodeBody, str]]]] = []
    for snapshot in intent.snapshots:
        async with self.authority.read(ctx) as view:
            members, originals, observation_ids = await self.source_snapshot_in(
                view, ctx, snapshot.source
            )
            if (
                tuple(record.reference for record in members) != snapshot.members
                or tuple(record.reference for record in originals) != snapshot.originals
                or observation_ids != snapshot.observations
            ):
                raise DomainError(
                    "episode_source_changed", "Source changed after rebuild admission"
                )
        planned: list[tuple[EpisodeBody, str]] = []
        for index, group in enumerate(groups(members, command)):
            catalog = {f"M{ordinal + 1}": record for ordinal, record in enumerate(group)}
            variables = EpisodeSegmentationInput(
                source=snapshot.source,
                members=tuple(
                    EpisodeSegmentationMember(
                        key=key,
                        reference=record.reference,
                        content=body_text(record.body),
                        event=TimeRange(
                            domain=record.times.event.domain,
                            start=record.times.event.coordinate,
                            end=record.times.event.coordinate,
                        )
                        if record.times.event
                        else None,
                    )
                    for key, record in catalog.items()
                ),
            )
            generated = await self.models.generate(
                ctx,
                "episode",
                variables.model_dump(),
                EpisodeSegmentationDraft,
                source_ids=tuple(
                    sorted({root for record in group for root in record.source_roots})
                ),
                invocation_id=f"episode:{command.operation_id}:{snapshot.source}:{index}",
            )
            assigned = [key for segment in generated.output.episodes for key in segment.member_keys]
            if Counter(assigned) != Counter(catalog.keys()):
                raise DomainError(
                    "episode_coverage",
                    "Every frozen member must occur exactly once",
                    call_id=generated.call_id,
                )
            for segment in generated.output.episodes:
                planned.append(
                    (
                        EpisodeBody(
                            title=segment.title,
                            summary=segment.summary,
                            track="source:" + snapshot.source,
                            experience=segment.experience,
                            members=tuple(
                                EpisodeMember(reference=catalog[key].reference, ordinal=ordinal)
                                for ordinal, key in enumerate(segment.member_keys)
                            ),
                            boundary_explanation=segment.boundary_explanation,
                            lineage=snapshot.originals,
                        ),
                        generated.call_id,
                    )
                )
        plans.append((snapshot, planned))
    async with self.authority.write(
        ctx,
        operation,
        operation_kind="episode.publish",
        scope=ctx.require_subject(),
        payload=intent,
    ) as tx:
        if tx.replayed:
            return tx.replay_result
        await require_subject_in(tx, ctx, "memory")
        results: list[dict[str, JsonValue]] = []
        for snapshot, planned in plans:
            members, originals, observation_ids = await self.source_snapshot_in(
                tx, ctx, snapshot.source
            )
            if (
                tuple(record.reference for record in members) != snapshot.members
                or tuple(record.reference for record in originals) != snapshot.originals
                or observation_ids != snapshot.observations
            ):
                raise DomainError(
                    "episode_source_changed", "Source changed before Episode publication"
                )
            created: list[CognitiveRef] = []
            for body, call_id in planned:
                record = await self.cognition.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=body,
                        evidence=tuple(
                            EvidenceLink(reference=member.reference) for member in body.members
                        ),
                        producer_call_id=call_id,
                    ),
                )
                created.append(record.reference)
            for old in originals:
                assert isinstance(old.body, EpisodeBody)
                record = await self.cognition.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        id=old.reference.id,
                        expected_revision=old.reference.revision,
                        body=old.body.model_copy(
                            update={"state": "resegmented", "successors": tuple(created)}
                        ),
                        evidence=(EvidenceLink(reference=old.reference, role="source_rebuild"),),
                        times=old.times,
                    ),
                    state=RevisionState(
                        cause=RevisionCause(
                            action="episode.rebuild",
                            actor=ctx.caller_id,
                            reason="Explicit complete-source rebuild",
                            parent=old.reference,
                        )
                    ),
                )
            results.append(
                {
                    "source": snapshot.source,
                    "episode_count": len(created),
                    "paragraph_count": len(members),
                    "references": [reference.model_dump(mode="json") for reference in created],
                }
            )
        value = tx.result(
            {"sources": results, "rebuilt": len(results), "commit_seq": tx.commit_seq}
        )
    if self.cognition.invalidate:
        await self.cognition.invalidate()
    return TypeAdapter(JsonValue).validate_python(value)
