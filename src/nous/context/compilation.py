"""Context compilation owned behavior."""

from __future__ import annotations

from .selection import SelectionRequest

from hashlib import sha256
from typing import TYPE_CHECKING
from uuid import uuid4
from datetime import UTC, datetime
from nous_sdk.contracts import (
    CallContext,
    CognitiveRecord,
    DomainError,
    MediaRecord,
    QueryRequest,
)
from nous_sdk.contracts.context import (
    CognitiveTarget,
    ContextCursor,
    ContextProjection,
    ContextSegment,
    EpistemicView,
    MediaTarget,
)
from nous_sdk.contracts.resources import ResourceRecord
from nous_sdk.contracts.source_objects import EvidenceRelationRecord, ObservationRecord
from sqlalchemy import select
from nous.cognition.lifecycle import selectable
from nous.platform.persistence.authority import canonical_json
from .models import sessions
from .selection import automatic_resident_in, segment_eligible_in

if TYPE_CHECKING:
    from .runtime import ContextRuntime


async def select_projection(
    self: ContextRuntime,
    ctx: CallContext,
    command: SelectionRequest,
    previous: ContextProjection | None = None,
) -> ContextProjection:
    selected_ctx = ctx
    policy = await self.policy(ctx)
    consumer = policy.consumers.get(command.consumer_id)
    if consumer is None:
        raise DomainError("context_consumer_unknown", "Consumer has no configured context policy")
    max_items = min(command.max_items, consumer.max_items)
    max_bytes = min(command.max_text_bytes, consumer.max_text_bytes)
    offered: list[ContextSegment] = []
    degradations: list[str] = []
    async with self.authority.read(selected_ctx) as view:
        from .ego_supply import segments_in

        offered.extend(await segments_in(self, view, selected_ctx, command.usage))
        from .lifecycle import session_key

        session_record = await view.connection.scalar(
            select(sessions.c.record).where(sessions.c.id == session_key(ctx, command.session_id))
        )
        if session_record is not None:
            from nous_sdk.contracts.context import ContextSession

            session = ContextSession.model_validate(session_record)
            if session.closed:
                raise DomainError("session_closed", "Context session is closed")
            for resident in [item for item in session.residents if item.state != "evicted"][
                : consumer.max_residents
            ]:
                try:
                    if isinstance(resident.target, CognitiveTarget):
                        record = await automatic_resident_in(
                            self, view, selected_ctx, resident.target.reference
                        )
                        if record is None:
                            degradations.append("resident_inactive")
                            continue
                        offered.append(await self.record_segment_in(view, selected_ctx, record))
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
                            text="\n\n".join(
                                value
                                for value in (
                                    work.content.purpose,
                                    work.content.context_text,
                                    "\n".join(work.content.unresolved_questions),
                                    canonical_json(work.content.constraints)
                                    if work.content.constraints is not None
                                    else "",
                                    "\n".join(work.content.resume_conditions),
                                )
                                if value
                            ),
                            sources=work.content.anchors,
                            required=True,
                            authority="task_directive",
                            provenance={"work_context_id": work.id, "revision": work.revision},
                        )
                    )
        for target in command.current_sources:
            if isinstance(target, CognitiveTarget):
                record = await self.cognition.get_in(view, selected_ctx, target.reference)
                offered.append(
                    (await self.record_segment_in(view, selected_ctx, record)).model_copy(
                        update={"required": True}
                    )
                )
            else:
                offered.append(
                    await self.media_segment_in(view, selected_ctx, target, required=True)
                )
    if command.usage.symbols is not None:
        symbol_selection = await self.symbols.select(
            selected_ctx, command.usage.symbols, express=command.usage.symbols_mode == "express"
        )
        async with self.authority.read(selected_ctx) as symbol_view:
            for record in symbol_selection.candidates:
                offered.append(await self.record_segment_in(symbol_view, selected_ctx, record))
        if symbol_selection.local_interpretation is not None:
            from nous_sdk.contracts import EpistemicStance

            local = symbol_selection.local_interpretation
            producer = symbol_selection.producer
            if producer is None:
                raise DomainError(
                    "symbol_provenance_missing",
                    "A generated local interpretation requires its model provenance",
                )
            sources = tuple(
                {
                    entry.reference.model_dump_json(): entry.reference
                    for entry in local.sources
                    if entry.reference is not None
                }.values()
            )
            offered.append(
                ContextSegment(
                    key="symbols-local:" + producer.output_digest,
                    text=canonical_json(local),
                    sources=sources,
                    dependencies=symbol_selection.dependencies,
                    authority="steward_interpretation",
                    epistemic=EpistemicStance(
                        classification="inferred", confidence=local.confidence
                    ),
                    provenance={
                        "supply": "local_symbol_interpretation",
                    },
                    producer=producer,
                )
            )
        degradations.extend(symbol_selection.degradations)
    query = command.query
    if query is None and command.usage.intent.strip():
        query = QueryRequest(
            text=command.usage.intent, limit=max_items, capability_policy="best_effort"
        )
    if query is not None and command.include_optional:
        found = await self.retrieval.query(selected_ctx, query)
        async with self.authority.read(selected_ctx) as view:
            for hit in found.hits:
                value = hit.object
                if isinstance(value, CognitiveRecord) and selectable(value):
                    offered.append(await self.record_segment_in(view, selected_ctx, value))
                elif isinstance(value, MediaRecord):
                    offered.append(
                        await self.media_segment_in(
                            view, selected_ctx, MediaTarget(reference=value.reference)
                        )
                    )
                elif isinstance(value, ObservationRecord):
                    offered.append(
                        await self.media_segment_in(
                            view, selected_ctx, MediaTarget(reference=value.source)
                        )
                    )
                elif isinstance(value, EvidenceRelationRecord):
                    target = await self.cognition.get_in(view, selected_ctx, value.target)
                    offered.append(await self.record_segment_in(view, selected_ctx, target))
                    if value.source is not None:
                        offered.append(
                            await self.media_segment_in(
                                view, selected_ctx, MediaTarget(reference=value.source)
                            )
                        )
                    elif value.evidence.reference is not None:
                        evidence = await self.cognition.get_in(
                            view, selected_ctx, value.evidence.reference
                        )
                        offered.append(await self.record_segment_in(view, selected_ctx, evidence))
                elif isinstance(value, ResourceRecord):
                    if value.snapshot is not None:
                        offered.append(
                            await self.media_segment_in(
                                view, selected_ctx, MediaTarget(reference=value.snapshot.source)
                            )
                        )
                    else:
                        degradations.append("resource_source_unavailable")
    present = {target.model_dump_json() for target in command.present_sources}
    offered = [
        segment
        for segment in offered
        if segment.required
        or not segment.sources
        or (not all((target.model_dump_json() in present for target in segment.sources)))
    ]
    filtered: dict[str, ContextSegment] = {}
    for segment in offered:
        admitted_kind = all(
            (
                consumer.allow_media
                if target.kind == "media"
                else not consumer.allowed_kinds or target.reference.kind in consumer.allowed_kinds
                for target in segment.sources
            )
        )
        if not admitted_kind:
            if segment.required:
                raise DomainError(
                    "context_source_forbidden", "Required source is outside the consumer policy"
                )
            degradations.append("optional_source_kind_omitted")
            continue
        duplicate = filtered.get(segment.key)
        if duplicate is None or segment.required and not duplicate.required:
            filtered[segment.key] = segment
    offered = [
        segment for segment in filtered.values() if command.include_optional or segment.required
    ]
    required = [segment for segment in offered if segment.required]
    if (
        len(required) > max_items
        or sum((len(segment.text.encode()) for segment in required)) > max_bytes
    ):
        raise DomainError(
            "context_required_budget", "Required context does not fit the consumer input budget"
        )
    from .steward import steward_select

    offered, steward_decision = await steward_select(self, selected_ctx, command, consumer, offered)
    track_id = sha256(
        f"{ctx.caller_id}:{ctx.namespace_id}:{command.session_id}:{command.consumer_id}:{command.instance_id}".encode()
    ).hexdigest()
    scope = selected_ctx.allowed_source_ids
    config_digest = sha256(
        canonical_json(
            {
                "usage": {
                    "view": command.usage.view,
                    "role": command.usage.role,
                    "audience": command.usage.audience,
                    "audience_entities": command.usage.audience_entities,
                },
                "stable_content": [
                    (segment.key, segment.text) for segment in offered if segment.required
                ],
                "scope": scope,
                "source_constraints": selected_ctx.source_constraints,
                "max_items": command.max_items,
                "max_text_bytes": command.max_text_bytes,
                "consumer_policy": consumer,
            }
        ).encode()
    ).hexdigest()
    async with self.authority.read(ctx) as tx:
        from .lifecycle import session_key as context_session_key

        session_key = context_session_key(ctx, command.session_id)
        prior_session = await tx.connection.scalar(
            select(sessions.c.record).where(
                sessions.c.id == session_key, sessions.c.namespace_id == ctx.namespace_id
            )
        )
        if prior_session is not None and prior_session.get("closed", False):
            raise DomainError("session_closed", "Context session is closed")
        old = previous
        append = (
            old is not None
            and old.cursor == command.known_cursor
            and (old.config_digest == config_digest)
            and (
                not (prior_session is not None and prior_session.get("history_invalidated", False))
            )
        )
        previous: tuple[ContextSegment, ...] = ()
        if append and old is not None:
            try:
                await self.require_projection_in(tx, selected_ctx, old)
                previous = old.segments
            except DomainError:
                append = False
        selected: list[ContextSegment] = []
        used = 0
        for segment in sorted(offered, key=lambda segment: not segment.required):
            if not await segment_eligible_in(self, tx, selected_ctx, segment):
                degradations.append("optional_source_inactive")
                continue
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
        append = append and tuple(selected[: len(previous)]) == previous
        delta = tuple(selected[len(previous) :]) if append else tuple(selected)
        projection_id = str(uuid4())
        cursor = ContextCursor(
            track_id=track_id,
            epoch_id=old.cursor.epoch_id if append and old is not None else str(uuid4()),
            revision=old.cursor.revision + bool(delta) if append and old is not None else 1,
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
                (
                    segment.key
                    for segment in selected
                    if segment.epistemic is not None
                    and segment.epistemic.classification == "narrative"
                )
            ),
            simulated_sources=tuple(
                (
                    segment.key
                    for segment in selected
                    if segment.epistemic is not None
                    and segment.epistemic.classification == "simulated"
                )
            ),
        )
        full = ContextProjection(
            id=projection_id,
            namespace_id=ctx.namespace_id,
            session_id=command.session_id,
            consumer_id=command.consumer_id,
            instance_id=command.instance_id,
            usage=command.usage,
            source_watermark=tx.commit_seq,
            segments=tuple(selected),
            epistemic_view=epistemic,
            cursor=cursor,
            config_digest=config_digest,
            recorded_at=datetime.now(UTC).isoformat(),
            commit_seq=tx.commit_seq,
            steward_decision=steward_decision,
            degradations=tuple(sorted(set(degradations))),
        )
        return full
