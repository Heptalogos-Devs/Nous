"""Episode management reads and durable source rebuilds over canonical revisions."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, CognitiveRecord, CognitiveRef, DomainError, EpisodeBody
from nous_sdk.contracts.episodes import EpisodeQuery, EpisodeView
from sqlalchemy import func, select

from nous.identity.service import require_subject_in
from nous.material.named_sources import named_source_observations_in, source_name
from nous.material.service import materials, observations, require_observation_in
from nous.platform.execution import DurableExecution
from nous.platform.persistence import Authority, ReadView, WriteTx

from . import episode_jobs, episode_rebuild
from .projection import body_text, references_in
from .schema import revisions

if TYPE_CHECKING:
    from .models import ModelServices
    from .service import CognitionService


class EpisodeService:
    submit_rebuild = episode_rebuild.submit_rebuild
    run_rebuild = episode_rebuild.run_rebuild
    status = episode_jobs.status
    process_sources = episode_jobs.process_sources

    async def source_names(self, ctx: CallContext) -> tuple[str, ...]:
        names: set[str] = set()
        async with self.authority.read(ctx) as view:
            await require_subject_in(view, ctx)
            for kind in ("memory", "episode"):
                for record in await self.heads_in(view, ctx, kind):
                    if (
                        not record.deleted
                        and not record.purged
                        and await self.cognition.visible_in(view, ctx, record)
                    ):
                        names.update(await self.names_in(view, ctx, record))
        return tuple(sorted(names))

    def __init__(
        self,
        authority: Authority,
        cognition: CognitionService,
        models: ModelServices,
        execution: DurableExecution,
    ):
        self.authority, self.cognition = authority, cognition
        self.models, self.execution = models, execution

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
            revisions.c.subject_id == ctx.require_subject(),
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
                        observations.c.subject_id == ctx.require_subject(),
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
            if record.deleted or record.purged or not selected.intersection(record.source_roots):
                continue
            members.append(await self.cognition.get_in(view, ctx, record.reference))
        originals: list[CognitiveRecord] = []
        for record in await self.heads_in(view, ctx, "episode", track="source:" + source):
            if (
                record.deleted
                or record.purged
                or not isinstance(record.body, EpisodeBody)
                or record.body.state != "active"
                or record.body.track != "source:" + source
                or not selected.intersection(record.source_roots)
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
                    observations.c.subject_id == ctx.subject_id,
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
            await require_subject_in(view, ctx)
            person_members: set[str] | None = None
            if command.person is not None:
                person = await self.cognition.get_in(view, ctx, command.person)
                if person.reference.kind != "entity":
                    raise DomainError("person_required", "Episode person filter requires an Entity")
                person_members = {reference.exact() for reference in references_in(person.body)}
            found: list[CognitiveRecord] = []
            for record in await self.heads_in(view, ctx, "episode"):
                body = record.body
                if (
                    record.deleted
                    or not isinstance(body, EpisodeBody)
                    or body.state != "active"
                    or not await self.cognition.visible_in(view, ctx, record)
                ):
                    continue
                if command.query.casefold() not in body_text(body).casefold():
                    continue
                if command.source is not None and command.source not in await self.names_in(
                    view, ctx, record
                ):
                    continue
                if person_members is not None and not person_members.intersection(
                    member.reference.exact() for member in body.members
                ):
                    continue
                wanted, actual = command.experience, body.experience
                if wanted is not None and (
                    wanted.domain != actual.domain
                    or (wanted.start is not None or wanted.end is not None)
                    and actual.start is None
                    and actual.end is None
                    or wanted.start is not None
                    and actual.end is not None
                    and actual.end < wanted.start
                    or wanted.end is not None
                    and actual.start is not None
                    and actual.start > wanted.end
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
            await require_subject_in(view, ctx)
            record = await self.cognition.get_in(view, ctx, reference)
            return await self.view_in(view, ctx, record, paragraph_limit)

    async def history(
        self, ctx: CallContext, episode_id: str, limit: int = 100
    ) -> tuple[CognitiveRecord, ...]:
        records = await self.cognition.history(ctx, episode_id, limit)
        if any(record.reference.kind != "episode" for record in records):
            raise DomainError("episode_required", "The identity must identify an Episode")
        return records
