"""Context compilation owned behavior."""

from __future__ import annotations

from hashlib import sha256
from typing import TYPE_CHECKING
from uuid import uuid4

from nous_wave.contracts import (
    AssociationBody,
    CallContext,
    CognitiveRecord,
    DomainError,
    EntityBody,
    EpistemicStance,
    JournalBody,
    MemoryBody,
    MemorySemanticsBody,
    NarrativeThreadBody,
    QueryRequest,
)
from nous_wave.contracts.context import (
    CognitiveTarget,
    ContextCursor,
    ContextPreparation,
    ContextPrepare,
    ContextProjection,
    ContextSegment,
    ContextTarget,
    ContextTrack,
    ContextTrackState,
    EpistemicView,
    ManagedContextResponse,
    MediaTarget,
)
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select, update

from nous.cognition.projection import body_text
from nous.platform.persistence import ReadView, WriteTx
from nous.platform.persistence.authority import canonical_json

from .models import preparations, sessions, tracks

if TYPE_CHECKING:
    from .managed import ManagedContexts


def track_identity(ctx: CallContext, command: ContextTrack | ContextPrepare) -> str:
    return sha256(
        f"{ctx.caller_id}\0{ctx.namespace_id}\0{ctx.require_subject()}\0{command.session_id}\0{command.consumer_id}\0{command.instance_id}".encode()
    ).hexdigest()


async def read_track(
    self: ManagedContexts, ctx: CallContext, command: ContextTrack
) -> ContextTrackState:
    async with self.authority.read(ctx) as view:
        raw = await view.connection.scalar(
            select(tracks.c.record).where(
                tracks.c.id == track_identity(ctx, command),
                tracks.c.namespace_id == ctx.namespace_id,
                tracks.c.subject_id == ctx.require_subject(),
            )
        )
        projection = ContextProjection.model_validate(raw) if raw is not None else None
        reset_reason = None
        if projection is not None:
            try:
                await self.require_targets_in(
                    view,
                    ctx,
                    tuple(
                        target
                        for segment in projection.segments
                        for target in (*segment.sources, *segment.dependencies)
                    ),
                )
            except DomainError as error:
                if error.code not in {
                    "source_unavailable",
                    "cognition_not_found",
                    "region_not_found",
                    "material_not_found",
                }:
                    raise
                projection, reset_reason = None, "source_unavailable"
        pending: list[str] = []
        values = (
            await view.connection.execute(
                select(preparations.c.record).where(
                    preparations.c.track_id == track_identity(ctx, command),
                    preparations.c.namespace_id == ctx.namespace_id,
                    preparations.c.subject_id == ctx.subject_id,
                )
            )
        ).scalars()
        for value in values:
            if value.get("caller_id") != ctx.caller_id:
                continue
            prepared = ContextPreparation.model_validate(value["preparation"])
            if prepared.state == "prepared":
                pending.append(prepared.id)
        return ContextTrackState(
            projection=projection, pending_preparation_ids=tuple(pending), reset_reason=reset_reason
        )


async def read_preparation_in(
    self: ManagedContexts, view: ReadView | WriteTx, ctx: CallContext, preparation_id: str
) -> dict[str, JsonValue]:
    value = (
        await view.connection.execute(
            select(preparations.c.record).where(
                preparations.c.id == preparation_id,
                preparations.c.namespace_id == ctx.namespace_id,
                preparations.c.subject_id == ctx.require_subject(),
            )
        )
    ).scalar_one_or_none()
    if value is None:
        raise DomainError("preparation_not_found", "Context preparation is unavailable")
    parsed = TypeAdapter(dict[str, JsonValue]).validate_python(value)
    if parsed.get("caller_id") != ctx.caller_id:
        raise DomainError(
            "preparation_forbidden", "Context preparation belongs to a different caller"
        )
    return parsed


async def require_targets_in(
    self: ManagedContexts,
    view: ReadView | WriteTx,
    ctx: CallContext,
    targets: tuple[ContextTarget, ...],
) -> None:
    for target in targets:
        if isinstance(target, CognitiveTarget):
            await self.cognition.get_in(view, ctx, target.reference)
        else:
            if self.materials is None:
                raise DomainError("media_unavailable", "Context media owner is unavailable")
            if target.reference.kind == "material":
                await self.materials.read_in(view, ctx, target.reference.id)
            elif target.reference.kind == "source_region":
                await self.materials.read_region_in(view, ctx, target.reference.id)
            elif self.derivations is not None:
                await self.derivations.authorize_in(view, ctx, target.reference)
            else:
                raise DomainError("source_unavailable", "Context media derivation is unavailable")


async def prepare(
    self: ManagedContexts, ctx: CallContext, command: ContextPrepare
) -> ContextPreparation:
    # Managed input begins with no implicit source access. Trusted host scopes
    # enter through the same authenticated boundary used by ordinary queries.
    selected_ctx = (
        ctx
        if ctx.allowed_source_ids is not None
        else ctx.model_copy(update={"allowed_source_ids": ()})
    )
    policy = await self.policy(ctx)
    consumer = policy.consumers.get(command.consumer_id)
    if consumer is None:
        raise DomainError("context_consumer_unknown", "Consumer has no configured context policy")
    max_items = min(command.max_items, consumer.max_items)
    max_bytes = min(command.max_text_bytes, consumer.max_text_bytes)
    offered: list[ContextSegment] = []
    degradations: list[str] = []
    async with self.authority.read(selected_ctx) as view:
        from .lifecycle import session_key

        session_record = await view.connection.scalar(
            select(sessions.c.record).where(sessions.c.id == session_key(ctx, command.session_id))
        )
        if session_record is not None:
            from nous_wave.contracts.context import ContextSession

            session = ContextSession.model_validate(session_record)
            if session.closed:
                raise DomainError("session_closed", "Context session is closed")
            for resident in [item for item in session.residents if item.state != "evicted"][
                : consumer.max_residents
            ]:
                try:
                    await self.require_targets_in(view, selected_ctx, (resident.target,))
                    if isinstance(resident.target, CognitiveTarget):
                        offered.append(
                            self.record_segment(
                                await self.cognition.get_in(
                                    view, selected_ctx, resident.target.reference
                                )
                            )
                        )
                    else:
                        offered.append(
                            await self.media_segment_in(view, selected_ctx, resident.target)
                        )
                except DomainError:
                    degradations.append("resident_unavailable")
            if session.active_work_context_id is not None:
                work = await self.work_in(view, selected_ctx, session.active_work_context_id)
                if work.state == "open":
                    offered.append(
                        ContextSegment(
                            key="work:" + work.id + "@" + str(work.revision),
                            text=canonical_json(work.content),
                            sources=work.content.anchors,
                            required=True,
                            authority="task_directive",
                            provenance={"work_context_id": work.id, "revision": work.revision},
                        )
                    )
        for target in command.current_sources:
            if isinstance(target, CognitiveTarget):
                record = await self.cognition.get_in(view, selected_ctx, target.reference)
                offered.append(self.record_segment(record).model_copy(update={"required": True}))
            else:
                offered.append(
                    await self.media_segment_in(view, selected_ctx, target, required=True)
                )
    query = command.query
    if query is None and command.usage.intent.strip():
        query = QueryRequest(text=command.usage.intent, mode="lexical", limit=max_items)
    if query is not None:
        if self.retrieval is None:
            raise DomainError("retrieval_unavailable", "Context retrieval owner is unavailable")
        found = await self.retrieval.query(selected_ctx, query)
        for hit in found.hits:
            if isinstance(hit.object, CognitiveRecord):
                offered.append(self.record_segment(hit.object))
    filtered: dict[str, ContextSegment] = {}
    for segment in offered:
        admitted_kind = all(
            consumer.allow_media
            if target.kind == "media"
            else not consumer.allowed_kinds or target.reference.kind in consumer.allowed_kinds
            for target in segment.sources
        )
        if not admitted_kind:
            if segment.required:
                raise DomainError(
                    "context_source_forbidden", "Required source is outside the consumer policy"
                )
            degradations.append("optional_source_kind_omitted")
            continue
        duplicate = filtered.get(segment.key)
        filtered[segment.key] = segment.model_copy(
            update={"required": segment.required or bool(duplicate and duplicate.required)}
        )
    offered = list(filtered.values())
    required = [segment for segment in offered if segment.required]
    if (
        len(required) > max_items
        or sum(len(segment.text.encode()) for segment in required) > max_bytes
    ):
        raise DomainError(
            "context_required_budget", "Required context does not fit the consumer input budget"
        )
    from .steward import steward_select

    offered, steward_call_id = await steward_select(self, selected_ctx, command, consumer, offered)
    track_id = track_identity(ctx, command)
    scope = selected_ctx.allowed_source_ids
    config_digest = sha256(
        canonical_json(
            {
                "usage": command.usage,
                "scope": scope,
                "source_constraints": selected_ctx.source_constraints,
                "max_items": command.max_items,
                "max_text_bytes": command.max_text_bytes,
                "consumer_policy": consumer,
                "steward_call_id": steward_call_id,
            }
        ).encode()
    ).hexdigest()
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="context.prepare",
        scope=ctx.require_subject(),
        payload={
            "command": command,
            "source_scope": scope,
            "source_constraints": selected_ctx.source_constraints,
        },
    ) as tx:
        if tx.replayed:
            replay = ContextPreparation.model_validate(tx.replay_result)
            stored = await self.read_preparation_in(tx, ctx, replay.id)
            full = ContextProjection.model_validate(stored["full_projection"])
            await self.require_targets_in(
                tx,
                selected_ctx,
                tuple(target for segment in full.segments for target in segment.sources),
            )
            return ContextPreparation.model_validate(stored["preparation"])
        session_key = sha256(
            f"{ctx.caller_id}\0{ctx.namespace_id}\0{ctx.require_subject()}\0{command.session_id}".encode()
        ).hexdigest()
        prior_session = (
            await tx.connection.execute(
                select(sessions.c.record).where(
                    sessions.c.id == session_key,
                    sessions.c.namespace_id == ctx.namespace_id,
                    sessions.c.subject_id == ctx.subject_id,
                )
            )
        ).scalar_one_or_none()
        if prior_session is not None and prior_session.get("closed", False):
            raise DomainError("session_closed", "Context session is closed")
        if prior_session is None:
            await tx.connection.execute(
                sessions.insert().values(
                    id=session_key,
                    namespace_id=ctx.namespace_id,
                    subject_id=ctx.subject_id,
                    record={
                        "id": command.session_id,
                        "closed": False,
                        "namespace_id": ctx.namespace_id,
                        "subject_id": ctx.subject_id,
                        "commit_seq": tx.commit_seq,
                        "recorded_at": tx.recorded_at,
                    },
                )
            )
        pending = (
            await tx.connection.execute(
                select(preparations.c.record).where(preparations.c.track_id == track_id)
            )
        ).scalars()
        if any(
            ContextPreparation.model_validate(value["preparation"]).state == "prepared"
            for value in pending
        ):
            raise DomainError(
                "context_pending",
                "Cancel or admit the pending context before preparing another input",
            )
        prior = (
            await tx.connection.execute(select(tracks.c.record).where(tracks.c.id == track_id))
        ).scalar_one_or_none()
        old = ContextProjection.model_validate(prior) if prior is not None else None
        append = (
            old is not None
            and old.cursor == command.known_cursor
            and old.config_digest == config_digest
            and not (prior_session is not None and prior_session.get("history_invalidated", False))
        )
        previous: tuple[ContextSegment, ...] = ()
        if append and old is not None:
            try:
                await self.require_targets_in(
                    tx,
                    selected_ctx,
                    tuple(target for segment in old.segments for target in segment.sources),
                )
                previous = old.segments
            except DomainError:
                append = False
        selected: list[ContextSegment] = []
        used = 0
        for segment in sorted(offered, key=lambda segment: not segment.required):
            await self.require_targets_in(tx, selected_ctx, segment.sources)
            size = len(segment.text.encode())
            if len(selected) >= max_items or used + size > max_bytes:
                if segment.required:
                    raise DomainError(
                        "context_required_budget",
                        "Required context does not fit the consumer input budget",
                    )
                degradations.append("optional_source_budget_omitted")
                continue
            selected.append(segment)
            used += size
        # An append must preserve the complete admitted prefix, including text,
        # authority and dependencies. A changed selection starts a new epoch.
        append = append and tuple(selected[: len(previous)]) == previous
        delta = tuple(selected[len(previous) :]) if append else tuple(selected)
        projection_id = str(uuid4())
        cursor = ContextCursor(
            track_id=track_id,
            epoch_id=old.cursor.epoch_id if append and old is not None else str(uuid4()),
            revision=(old.cursor.revision + bool(delta)) if append and old is not None else 1,
            projection_id=projection_id,
        )
        classes = tuple(
            sorted(
                {
                    segment.epistemic.classification
                    for segment in selected
                    if segment.epistemic is not None
                }
            )
        )
        epistemic = EpistemicView(
            classifications=classes,
            narrative_sources=tuple(
                segment.key
                for segment in selected
                if segment.epistemic is not None and segment.epistemic.classification == "narrative"
            ),
            simulated_sources=tuple(
                segment.key
                for segment in selected
                if segment.epistemic is not None and segment.epistemic.classification == "simulated"
            ),
        )
        full = ContextProjection(
            id=projection_id,
            namespace_id=ctx.namespace_id,
            subject_id=ctx.require_subject(),
            session_id=command.session_id,
            consumer_id=command.consumer_id,
            instance_id=command.instance_id,
            usage=command.usage,
            source_watermark=tx.commit_seq,
            segments=tuple(selected),
            epistemic_view=epistemic,
            cursor=cursor,
            config_digest=config_digest,
            recorded_at=tx.recorded_at,
            commit_seq=tx.commit_seq,
            steward_call_id=steward_call_id,
            degradations=tuple(sorted(set(degradations))),
        )
        response = ManagedContextResponse(
            kind="append" if append else "reset",
            cursor=cursor,
            projection=full.model_copy(update={"segments": delta}),
        )
        preparation = ContextPreparation(
            id=str(uuid4()), state="prepared", request=command, response=response
        )
        value = {
            "preparation": preparation.model_dump(mode="json"),
            "full_projection": full.model_dump(mode="json"),
            "source_scope": scope,
            "source_constraints": selected_ctx.source_constraints,
            "caller_id": ctx.caller_id,
        }
        await tx.connection.execute(
            preparations.insert().values(
                id=preparation.id,
                namespace_id=ctx.namespace_id,
                subject_id=ctx.subject_id,
                track_id=track_id,
                record=value,
            )
        )
        return tx.result(preparation)


def record_segment(record: CognitiveRecord) -> ContextSegment:
    if record.body is None:
        raise DomainError("source_purged", "Purged cognitive content cannot enter a context")
    if isinstance(record.body, (MemoryBody, MemorySemanticsBody, AssociationBody)):
        epistemic = record.body.epistemic
    elif isinstance(record.body, JournalBody):
        epistemic = record.body.perspective
    elif isinstance(record.body, NarrativeThreadBody):
        epistemic = EpistemicStance(
            classification="narrative", attribution=record.reference.exact()
        )
    else:
        epistemic = None
    return ContextSegment(
        key=record.reference.exact(),
        text=canonical_json(record.body)
        if isinstance(record.body, EntityBody)
        else body_text(record.body),
        sources=(CognitiveTarget(reference=record.reference),),
        dependencies=tuple(
            CognitiveTarget(reference=edge.reference)
            for edge in record.evidence
            if edge.reference is not None
        ),
        authority="recorded_cognition",
        provenance={
            "source_roots": list(record.source_roots),
            "revision": record.reference.revision,
        },
        epistemic=epistemic,
    )


async def preparation(
    self: ManagedContexts, ctx: CallContext, preparation_id: str
) -> ContextPreparation:
    async with self.authority.read(ctx) as view:
        value = await self.read_preparation_in(view, ctx, preparation_id)
        full = ContextProjection.model_validate(value["full_projection"])
        await self.require_targets_in(
            view, ctx, tuple(target for segment in full.segments for target in segment.sources)
        )
        return ContextPreparation.model_validate(value["preparation"])


async def cancel_preparation(
    self: ManagedContexts, ctx: CallContext, preparation_id: str, operation_id: str
) -> ContextPreparation:
    async with self.authority.write(
        ctx,
        operation_id,
        operation_kind="context.cancel_preparation",
        scope=ctx.require_subject(),
        payload={"id": preparation_id},
    ) as tx:
        if tx.replayed:
            return ContextPreparation.model_validate(tx.replay_result)
        value = await self.read_preparation_in(tx, ctx, preparation_id)
        preparation = ContextPreparation.model_validate(value["preparation"])
        if preparation.state == "admitted":
            raise DomainError("context_already_admitted", "Admitted context cannot be cancelled")
        empty = preparation.response.projection.model_copy(update={"segments": ()})
        result = preparation.model_copy(
            update={
                "state": "cancelled",
                "response": preparation.response.model_copy(update={"projection": empty}),
            }
        )
        value["full_projection"] = TypeAdapter(JsonValue).validate_json(empty.model_dump_json())
        value["preparation"] = TypeAdapter(JsonValue).validate_json(result.model_dump_json())
        await tx.connection.execute(
            update(preparations).where(preparations.c.id == preparation_id).values(record=value)
        )
        return tx.result(result)


async def media_segment_in(
    self: ManagedContexts,
    view: ReadView | WriteTx,
    ctx: CallContext,
    target: MediaTarget,
    *,
    required: bool = False,
) -> ContextSegment:
    from nous_wave.contracts.derivation import StructuredRepresentation, TextRepresentation

    await self.require_targets_in(view, ctx, (target,))
    reference = target.reference
    if reference.kind in {"material", "source_region"}:
        if self.materials is None:
            raise DomainError("media_unavailable", "Context media owner is unavailable")
        material, region, observation_id = await self.materials.media_source_in(
            view, ctx, reference
        )
        text = material["text"]
        if text is not None and region is not None:
            from nous.material.text import source_region_text

            text = (
                source_region_text(text, region)
                if any(
                    value is not None
                    for value in (
                        region.text_start,
                        region.text_end,
                        region.json_pointer,
                        region.line_number,
                    )
                )
                else None
            )
        provenance = {
            "media_reference": reference.model_dump(mode="json"),
            "source_roots": [observation_id],
            "media_type": material["media_type"],
        }
        epistemic = None
    else:
        if self.derivations is None:
            raise DomainError("media_unavailable", "Context derivation owner is unavailable")
        selected = await self.derivations.selected_in(view, ctx, reference)
        info = await self.derivations.source_in(view, ctx, reference)
        if isinstance(selected.payload, TextRepresentation):
            text = selected.payload.text
        elif isinstance(selected.payload, StructuredRepresentation):
            text = canonical_json(selected.payload.value)
        else:
            text = canonical_json(selected.payload)
        provenance = {
            "media_reference": reference.model_dump(mode="json"),
            "source_roots": list(info.roots),
            "source_channel_access": info.access.model_dump(mode="json"),
        }
        epistemic = EpistemicStance(classification="derived", attribution=reference.exact())
    return ContextSegment(
        key=reference.exact(),
        text=text or reference.exact(),
        sources=(target,),
        required=required,
        authority="source_material",
        provenance=TypeAdapter(dict[str, JsonValue]).validate_python(provenance),
        epistemic=epistemic,
    )
