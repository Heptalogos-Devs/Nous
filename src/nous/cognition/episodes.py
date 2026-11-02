"""Episode management reads and durable source rebuilds over canonical revisions."""

from __future__ import annotations

from nous_sdk.presentation import body_text

from collections import Counter
from typing import TYPE_CHECKING

from nous_sdk.contracts import (
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
from nous_sdk.contracts.episodes import (
    EpisodeQuery,
    EpisodeRebuildIntent,
    EpisodeRebuildSubmission,
    EpisodeSegmentationDraft,
    EpisodeSegmentationInput,
    EpisodeSegmentationMember,
    EpisodeSourceSnapshot,
    EpisodeView,
    ProcessEpisodeSources,
    RebuildEpisodes,
)
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import func, select

from nous.identity.access import require_namespace_in
from nous.material.named_sources import named_source_observations_in, source_name
from nous.material.service import materials, observations, require_observation_in
from nous.platform.execution import DurableExecution, JobSpec
from nous.platform.persistence import Authority, ReadView, WriteTx

from .lifecycle import selectable
from .projection import references_in
from .revision_state import RevisionState
from .schema import revisions

if TYPE_CHECKING:
    from .models import ModelServices
    from .records import CognitiveRecords


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
            or (bool(point[0]) and point[1] > previous[1] + command.group_gap_ms)
        )
        if current and (
            boundary
            or len(current) >= command.max_members_per_call
            or size + length > command.max_characters_per_call
        ):
            result.append(current)
            current, size = ([], 0)
        current.append(record)
        size += length
        previous = point
    if current:
        result.append(current)
    return result


class EpisodeService:
    async def source_names(self, ctx: CallContext) -> tuple[str, ...]:
        names: set[str] = set()
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            for kind in ("memory", "episode"):
                for record in await self.heads_in(view, ctx, kind):
                    if selectable(record) and await self.cognition.visible_in(view, ctx, record):
                        names.update(await self.names_in(view, ctx, record))
        return tuple(sorted(names))

    def __init__(
        self,
        authority: Authority,
        cognition: CognitiveRecords,
        models: ModelServices,
        execution: DurableExecution,
    ):
        self.authority, self.cognition = (authority, cognition)
        self.models, self.execution = (models, execution)

    async def heads_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        kind: str,
        *,
        source_roots: frozenset[str] | None = None,
        track: str | None = None,
    ) -> tuple[CognitiveRecord, ...]:
        statement = select(revisions.c.record).where(
            revisions.c.namespace_id == ctx.namespace_id,
            revisions.c.kind == kind,
            revisions.c.head.is_(True),
        )
        if source_roots is not None:
            roots = func.json_each(revisions.c.record, "$.source_roots").table_valued("value")
            statement = statement.where(
                select(roots.c.value).where(roots.c.value.in_(source_roots)).exists()
            )
        if track is not None:
            statement = statement.where(revisions.c.record["body"]["track"].as_string() == track)
        return tuple(
            CognitiveRecord.model_validate(value)
            for value in (await view.connection.execute(statement)).scalars()
        )

    async def source_snapshot_in(
        self, view: ReadView | WriteTx, ctx: CallContext, source: str
    ) -> tuple[tuple[CognitiveRecord, ...], tuple[CognitiveRecord, ...], tuple[str, ...]]:
        """A complete-source rebuild requires every actual source event, never a read subset."""
        if not source:
            raise DomainError("episode_source_required", "A source name must be nonempty")
        events = frozenset(
            str(value)
            for value in (
                await view.connection.execute(
                    select(observations.c.id).where(
                        observations.c.namespace_id == ctx.namespace_id,
                        observations.c.active.is_(True),
                    )
                )
            ).scalars()
        )
        selected = await named_source_observations_in(
            view, ctx, (source,), authorized_observation_ids=events
        )
        if not selected:
            raise DomainError("episode_source_missing", "No live source matches the exact name")
        for identifier in selected:
            await require_observation_in(view, ctx, identifier)
        members: list[CognitiveRecord] = []
        for record in await self.heads_in(view, ctx, "memory", source_roots=selected):
            if not selectable(record) or not selected.intersection(record.source_roots):
                continue
            members.append(await self.cognition.get_in(view, ctx, record.reference))
        originals: list[CognitiveRecord] = []
        for record in await self.heads_in(view, ctx, "episode", track="source:" + source):
            if (
                record.deleted
                or record.purged
                or (not isinstance(record.body, EpisodeBody))
                or (record.body.state != "active")
                or (record.body.track != "source:" + source)
                or (not selected.intersection(record.source_roots))
            ):
                continue
            originals.append(await self.cognition.get_in(view, ctx, record.reference))

        def order(record: CognitiveRecord) -> str:
            return record.reference.exact()

        return (
            tuple(sorted(members, key=order)),
            tuple(sorted(originals, key=order)),
            tuple(sorted(selected)),
        )

    async def names_in(
        self, view: ReadView | WriteTx, ctx: CallContext, record: CognitiveRecord
    ) -> tuple[str, ...]:
        if not record.source_roots:
            return ()
        rows = (
            await view.connection.execute(
                select(observations.c.record, materials.c.record.label("material_record"))
                .join(materials, materials.c.id == observations.c.material_id)
                .where(
                    observations.c.namespace_id == ctx.namespace_id,
                    observations.c.id.in_(record.source_roots),
                )
            )
        ).mappings()
        return tuple(
            sorted(
                {
                    name
                    for row in rows
                    if (name := source_name(row["record"], row["material_record"])) is not None
                }
            )
        )

    async def query(self, ctx: CallContext, command: EpisodeQuery) -> tuple[EpisodeView, ...]:
        async with self.authority.read(ctx) as view:
    
            await require_namespace_in(view, ctx, "episodes")
            await require_namespace_in(view, ctx)
            person_members: set[str] | None = None
            if command.person is not None:
                person = await self.cognition.get_in(view, ctx, command.person)
                if person.reference.kind != "entity":
                    raise DomainError("person_required", "Episode person filter requires an Entity")
                person_members = {reference.id for reference in references_in(person.body)}
            found: list[CognitiveRecord] = []
            for record in await self.heads_in(view, ctx, "episode"):
                body = record.body
                if (
                    record.deleted
                    or not isinstance(body, EpisodeBody)
                    or body.state != "active"
                    or (not await self.cognition.visible_in(view, ctx, record))
                ):
                    continue
                if command.query.casefold() not in body_text(body).casefold():
                    continue
                if command.track is not None and body.track != command.track:
                    continue
                if command.source is not None and command.source not in await self.names_in(
                    view, ctx, record
                ):
                    continue
                if person_members is not None and (
                    not person_members.intersection(member.reference.id for member in body.members)
                ):
                    continue
                wanted, actual = (command.experience, body.experience)
                if wanted is not None and (
                    wanted.domain != actual.domain
                    or (
                        (wanted.start is not None or wanted.end is not None)
                        and actual.start is None
                        and (actual.end is None)
                    )
                    or (
                        wanted.start is not None
                        and actual.end is not None
                        and (actual.end <= wanted.start)
                    )
                    or (
                        wanted.end is not None
                        and actual.start is not None
                        and (actual.start >= wanted.end)
                    )
                ):
                    continue
                found.append(record)
            found.sort(
                key=lambda record: (record.commit_seq, record.reference.exact()), reverse=True
            )
            return tuple(
                [await self.view_in(view, ctx, record) for record in found[: command.limit]]
            )

    async def view_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        record: CognitiveRecord,
        paragraph_limit: int = 1000,
    ) -> EpisodeView:
        if not isinstance(record.body, EpisodeBody):
            raise DomainError("episode_required", "The reference must identify an Episode")
        members = tuple(
            [
                await self.cognition.get_in(view, ctx, member.reference)
                for member in sorted(record.body.members, key=lambda member: member.ordinal)[
                    :paragraph_limit
                ]
            ]
        )
        return EpisodeView(
            record=record, members=members, sources=await self.names_in(view, ctx, record)
        )

    async def get(
        self, ctx: CallContext, reference: CognitiveRef, paragraph_limit: int = 1000
    ) -> EpisodeView:
        if not 1 <= paragraph_limit <= 1000:
            raise DomainError("invalid_limit", "Paragraph limit must be between 1 and 1000")
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            record = await self.cognition.get_in(view, ctx, reference)
            return await self.view_in(view, ctx, record, paragraph_limit)

    async def history(
        self, ctx: CallContext, episode_id: str, limit: int = 100
    ) -> tuple[CognitiveRecord, ...]:
        records = await self.cognition.history(ctx, episode_id, limit)
        if any(record.reference.kind != "episode" for record in records):
            raise DomainError("episode_required", "The identity must identify an Episode")
        return records

    async def submit_rebuild(
        self, ctx: CallContext, command: RebuildEpisodes
    ) -> EpisodeRebuildSubmission:
        if len(set(command.sources)) != len(command.sources):
            raise DomainError("episode_source_duplicate", "Source names must be distinct")
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="episode.rebuild",
            scope=ctx.namespace_id,
            payload=command,
        ) as tx:
            if tx.replayed:
                result = EpisodeRebuildSubmission.model_validate(tx.replay_result)
                for source in result.sources:
                    await self.source_snapshot_in(tx, ctx, source)
                return result
            await require_namespace_in(tx, ctx, "memory")
    
            await require_namespace_in(tx, ctx, "episodes")
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
                    "nous.episodes.rebuild", "formation", args=(intent.model_dump(mode="json"),)
                )
            )
            return tx.result(
                EpisodeRebuildSubmission(job_id=job.workflow_id, sources=command.sources)
            )

    async def run_rebuild(self, payload: JsonValue) -> JsonValue:
        intent = EpisodeRebuildIntent.model_validate(payload)
        ctx, command = (intent.context, intent.command)
        operation = command.operation_id + ":publish"
        receipt = await self.authority.receipt(
            ctx, operation, operation_kind="episode.publish", scope=ctx.namespace_id, payload=intent
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
                        (
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
                        )
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
                assigned = [
                    key for segment in generated.output.episodes for key in segment.member_keys
                ]
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
                                    (
                                        EpisodeMember(
                                            reference=catalog[key].reference, ordinal=ordinal
                                        )
                                        for ordinal, key in enumerate(segment.member_keys)
                                    )
                                ),
                                boundary_explanation=segment.boundary_explanation,
                                lineage=snapshot.originals,
                            ),
                            generated.call_id,
                        )
                    )
            plans.append((snapshot, planned))
        async with self.authority.write(
            ctx, operation, operation_kind="episode.publish", scope=ctx.namespace_id, payload=intent
        ) as tx:
            if tx.replayed:
                return tx.replay_result
            await require_namespace_in(tx, ctx, "memory")
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
                            evidence=(
                                EvidenceLink(reference=old.reference, role="source_rebuild"),
                            ),
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
        return TypeAdapter(JsonValue).validate_python(value)

    async def status(self, ctx: CallContext, limit: int = 20) -> dict[str, JsonValue]:
        return await self.execution.summarize_jobs(ctx, kind="nous.episodes.rebuild", limit=limit)

    async def process_sources(
        self, ctx: CallContext, command: ProcessEpisodeSources
    ) -> dict[str, JsonValue]:
        batch = await self.execution.process_jobs(
            ctx, kind="nous.episodes.rebuild", limit=command.limit, max_retry=command.max_retry
        )
        completed = batch["items"]
        assert isinstance(completed, list)
        results = [item["result"] for item in completed if isinstance(item, dict)]
        rebuilt = sum(
            count
            for result in results
            if isinstance(result, dict) and isinstance((count := result.get("rebuilt")), int)
        )
        return {
            "processed": batch["processed"],
            "unfinished": batch["unfinished"],
            "rebuilt": rebuilt,
            "failed": batch["failed"],
            "failures": batch["failures"],
            "results": results,
        }
