"""Context compilation owned behavior."""
from __future__ import annotations
from nous_sdk.presentation import body_text
from hashlib import sha256
from typing import TYPE_CHECKING
from uuid import uuid4
from nous_sdk.contracts import AssociationBody, CallContext, CognitiveRecord, ConcernBody, DesireBody, DomainError, EntityBody, EpistemicStance, JournalBody, MediaRecord, MemoryBody, MemorySemanticsBody, NarrativeThreadBody, QueryRequest
from nous_sdk.contracts.context import CognitiveTarget, ContextCursor, ContextPreparation, ContextPrepare, ContextProjection, ContextSegment, ContextTarget, EpistemicView, ManagedContextResponse, MediaTarget
from nous_sdk.contracts.resources import ResourceRecord
from nous_sdk.contracts.source_objects import EvidenceRelationRecord, ObservationRecord
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select, update
from nous.cognition.lifecycle import selectable
from nous.cognition.projection import evidence_in
from nous.platform.persistence import ReadView, WriteTx
from nous.platform.persistence.authority import canonical_json
from .models import preparations, sessions, tracks
from .selection import automatic_resident_in, segment_eligible_in
from .tracking import track_identity
if TYPE_CHECKING:
    from .managed import ManagedContexts

async def read_preparation_in(self: ManagedContexts, view: ReadView | WriteTx, ctx: CallContext, preparation_id: str) -> dict[str, JsonValue]:
    value = (await view.connection.execute(select(preparations.c.record).where(preparations.c.id == preparation_id, preparations.c.namespace_id == ctx.namespace_id))).scalar_one_or_none()
    if value is None:
        raise DomainError('preparation_not_found', 'Context preparation is unavailable')
    parsed = TypeAdapter(dict[str, JsonValue]).validate_python(value)
    if parsed.get('caller_id') != ctx.caller_id:
        raise DomainError('preparation_forbidden', 'Context preparation belongs to a different caller')
    return parsed

async def require_targets_in(self: ManagedContexts, view: ReadView | WriteTx, ctx: CallContext, targets: tuple[ContextTarget, ...]) -> None:
    for target in targets:
        if isinstance(target, CognitiveTarget):
            await self.cognition.read_in(view, ctx, target.reference)
        elif target.reference.kind == 'material':
            await self.materials.read_in(view, ctx, target.reference.id)
        elif target.reference.kind == 'source_region':
            await self.materials.read_region_in(view, ctx, target.reference.id)
        else:
            await self.derivations.authorize_in(view, ctx, target.reference)

async def require_projection_in(self: ManagedContexts, view: ReadView | WriteTx, ctx: CallContext, projection: ContextProjection) -> None:
    await self.require_targets_in(view, ctx, tuple((target for segment in projection.segments for target in (*segment.sources, *segment.dependencies))))
    for segment in projection.segments:
        if segment.authority != 'recorded_cognition':
            continue
        for target in segment.sources:
            if not isinstance(target, CognitiveTarget) or target.reference.kind != 'entity' or target.reference.exact() != segment.key:
                continue
            current = await self.cognition.read_in(view, ctx, target.reference)
            expected = await self.record_segment_in(view, ctx, current)
            if expected.text != segment.text or expected.dependencies != segment.dependencies:
                raise DomainError('context_source_changed', "The Entity's supported fields changed; prepare the context again")

async def stored_projection_in(self: ManagedContexts, view: ReadView | WriteTx, ctx: CallContext, projection_id: str) -> ContextProjection:
    raw = await view.connection.scalar(select(preparations.c.record).where(preparations.c.namespace_id == ctx.namespace_id, preparations.c.record['full_projection']['id'].as_string() == projection_id))
    if raw is None or raw.get('caller_id') != ctx.caller_id:
        raise DomainError('preparation_not_found', 'Context projection is unavailable for this caller')
    return ContextProjection.model_validate(raw['full_projection'])

async def prepare(self: ManagedContexts, ctx: CallContext, command: ContextPrepare) -> ContextPreparation:
    selected_ctx = ctx
    policy = await self.policy(ctx)
    consumer = policy.consumers.get(command.consumer_id)
    if consumer is None:
        raise DomainError('context_consumer_unknown', 'Consumer has no configured context policy')
    max_items = min(command.max_items, consumer.max_items)
    max_bytes = min(command.max_text_bytes, consumer.max_text_bytes)
    offered: list[ContextSegment] = []
    degradations: list[str] = []
    async with self.authority.read(selected_ctx) as view:
        from .ego_supply import segments_in
        offered.extend(await segments_in(self, view, selected_ctx, command.usage))
        from .lifecycle import session_key
        session_record = await view.connection.scalar(select(sessions.c.record).where(sessions.c.id == session_key(ctx, command.session_id)))
        if session_record is not None:
            from nous_sdk.contracts.context import ContextSession
            session = ContextSession.model_validate(session_record)
            if session.closed:
                raise DomainError('session_closed', 'Context session is closed')
            for resident in [item for item in session.residents if item.state != 'evicted'][:consumer.max_residents]:
                try:
                    if isinstance(resident.target, CognitiveTarget):
                        record = await automatic_resident_in(self, view, selected_ctx, resident.target.reference)
                        if record is None:
                            degradations.append('resident_inactive')
                            continue
                        offered.append(await self.record_segment_in(view, selected_ctx, record))
                    else:
                        offered.append(await self.media_segment_in(view, selected_ctx, resident.target))
                except DomainError:
                    degradations.append('resident_unavailable')
            if session.active_work_context_id is not None:
                work = await self.work_in(view, selected_ctx, session.active_work_context_id)
                if work.state == 'open':
                    offered.append(ContextSegment(key='work:' + work.id + '@' + str(work.revision), text='\n\n'.join(value for value in (work.content.purpose,work.content.context_text,'\n'.join(work.content.unresolved_questions),canonical_json(work.content.constraints) if work.content.constraints is not None else '', '\n'.join(work.content.resume_conditions)) if value), sources=work.content.anchors, required=True, authority='task_directive', provenance={'work_context_id': work.id, 'revision': work.revision}))
        for target in command.current_sources:
            if isinstance(target, CognitiveTarget):
                record = await self.cognition.get_in(view, selected_ctx, target.reference)
                offered.append((await self.record_segment_in(view, selected_ctx, record)).model_copy(update={'required': True}))
            else:
                offered.append(await self.media_segment_in(view, selected_ctx, target, required=True))
    if command.usage.symbols is not None:
        symbol_selection = await self.symbols.select(selected_ctx, command.usage.symbols, express=command.usage.symbols_mode == 'express')
        async with self.authority.read(selected_ctx) as symbol_view:
            for record in symbol_selection.candidates:
                offered.append(await self.record_segment_in(symbol_view, selected_ctx, record))
        if symbol_selection.local_interpretation is not None:
            from nous_sdk.contracts import EpistemicStance
            local = symbol_selection.local_interpretation
            sources = tuple({entry.reference.model_dump_json(): entry.reference for entry in local.sources if entry.reference is not None}.values())
            offered.append(ContextSegment(key='symbols-local:' + str(symbol_selection.producer_call_id), text=canonical_json(local), sources=sources, dependencies=symbol_selection.dependencies, authority='steward_interpretation', epistemic=EpistemicStance(classification='inferred', confidence=local.confidence), provenance={'supply': 'local_symbol_interpretation', 'producer_call_id': symbol_selection.producer_call_id}))
        degradations.extend(symbol_selection.degradations)
    query = command.query
    if query is None and command.usage.intent.strip():
        query = QueryRequest(text=command.usage.intent, limit=max_items, capability_policy='best_effort')
    if query is not None and command.include_optional:
        found = await self.retrieval.query(selected_ctx, query)
        async with self.authority.read(selected_ctx) as view:
            for hit in found.hits:
                value = hit.object
                if isinstance(value, CognitiveRecord) and selectable(value):
                    offered.append(await self.record_segment_in(view, selected_ctx, value))
                elif isinstance(value, MediaRecord):
                    offered.append(await self.media_segment_in(view, selected_ctx, MediaTarget(reference=value.reference)))
                elif isinstance(value, ObservationRecord):
                    offered.append(await self.media_segment_in(view, selected_ctx, MediaTarget(reference=value.source)))
                elif isinstance(value, EvidenceRelationRecord):
                    target = await self.cognition.get_in(view, selected_ctx, value.target)
                    offered.append(await self.record_segment_in(view, selected_ctx, target))
                    if value.source is not None:
                        offered.append(await self.media_segment_in(view, selected_ctx, MediaTarget(reference=value.source)))
                    elif value.evidence.reference is not None:
                        evidence = await self.cognition.get_in(view, selected_ctx, value.evidence.reference)
                        offered.append(await self.record_segment_in(view, selected_ctx, evidence))
                elif isinstance(value, ResourceRecord):
                    if value.snapshot is not None:
                        offered.append(await self.media_segment_in(view, selected_ctx, MediaTarget(reference=value.snapshot.source)))
                    else:
                        degradations.append('resource_source_unavailable')
    present = {target.model_dump_json() for target in command.present_sources}
    offered = [segment for segment in offered if segment.required or not segment.sources or (not all((target.model_dump_json() in present for target in segment.sources)))]
    filtered: dict[str, ContextSegment] = {}
    for segment in offered:
        admitted_kind = all((consumer.allow_media if target.kind == 'media' else not consumer.allowed_kinds or target.reference.kind in consumer.allowed_kinds for target in segment.sources))
        if not admitted_kind:
            if segment.required:
                raise DomainError('context_source_forbidden', 'Required source is outside the consumer policy')
            degradations.append('optional_source_kind_omitted')
            continue
        duplicate = filtered.get(segment.key)
        filtered[segment.key] = segment.model_copy(update={'required': segment.required or bool(duplicate and duplicate.required)})
    offered = [segment for segment in filtered.values() if command.include_optional or segment.required]
    required = [segment for segment in offered if segment.required]
    if len(required) > max_items or sum((len(segment.text.encode()) for segment in required)) > max_bytes:
        raise DomainError('context_required_budget', 'Required context does not fit the consumer input budget')
    from .steward import steward_select
    offered, steward_call_id, steward_decision = await steward_select(self, selected_ctx, command, consumer, offered)
    track_id = track_identity(ctx, command)
    scope = selected_ctx.allowed_source_ids
    config_digest = sha256(canonical_json({'usage': {'view': command.usage.view, 'role': command.usage.role, 'audience': command.usage.audience, 'audience_entities': command.usage.audience_entities}, 'stable_content': [(segment.key, segment.text) for segment in offered if segment.required], 'scope': scope, 'source_constraints': selected_ctx.source_constraints, 'max_items': command.max_items, 'max_text_bytes': command.max_text_bytes, 'consumer_policy': consumer}).encode()).hexdigest()
    async with self.authority.write(ctx, command.operation_id, operation_kind='context.prepare', scope=ctx.namespace_id, payload={'command': command, 'source_scope': scope, 'source_constraints': selected_ctx.source_constraints}) as tx:
        if tx.replayed:
            replay = ContextPreparation.model_validate(tx.replay_result)
            stored = await self.read_preparation_in(tx, ctx, replay.id)
            full = ContextProjection.model_validate(stored['full_projection'])
            await self.require_projection_in(tx, selected_ctx, full)
            return ContextPreparation.model_validate(stored['preparation'])
        from .lifecycle import session_key as context_session_key
        session_key = context_session_key(ctx, command.session_id)
        prior_session = (await tx.connection.execute(select(sessions.c.record).where(sessions.c.id == session_key, sessions.c.namespace_id == ctx.namespace_id))).scalar_one_or_none()
        if prior_session is not None and prior_session.get('closed', False):
            raise DomainError('session_closed', 'Context session is closed')
        if prior_session is None:
            await tx.connection.execute(sessions.insert().values(id=session_key, namespace_id=ctx.namespace_id, record={'id': command.session_id, 'closed': False, 'namespace_id': ctx.namespace_id, 'commit_seq': tx.commit_seq, 'recorded_at': tx.recorded_at}))
        pending = (await tx.connection.execute(select(preparations.c.record).where(preparations.c.track_id == track_id))).scalars()
        if any((ContextPreparation.model_validate(value['preparation']).state == 'prepared' for value in pending)):
            raise DomainError('context_pending', 'Cancel or admit the pending context before preparing another input')
        prior = (await tx.connection.execute(select(tracks.c.record).where(tracks.c.id == track_id))).scalar_one_or_none()
        old = ContextProjection.model_validate(prior) if prior is not None else None
        append = old is not None and old.cursor == command.known_cursor and (old.config_digest == config_digest) and (not (prior_session is not None and prior_session.get('history_invalidated', False)))
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
                degradations.append('optional_source_inactive')
                continue
            size = len(segment.text.encode())
            if len(selected) >= max_items or used + size > max_bytes:
                if segment.required:
                    raise DomainError('context_required_budget', 'Required context does not fit the consumer input budget')
                degradations.append('optional_source_budget_omitted')
                continue
            selected.append(segment)
            used += size
        append = append and tuple(selected[:len(previous)]) == previous
        delta = tuple(selected[len(previous):]) if append else tuple(selected)
        projection_id = str(uuid4())
        cursor = ContextCursor(track_id=track_id, epoch_id=old.cursor.epoch_id if append and old is not None else str(uuid4()), revision=old.cursor.revision + bool(delta) if append and old is not None else 1, projection_id=projection_id)
        classes = tuple(sorted({segment.epistemic.classification for segment in selected if segment.epistemic is not None}))
        epistemic = EpistemicView(classifications=classes, narrative_sources=tuple((segment.key for segment in selected if segment.epistemic is not None and segment.epistemic.classification == 'narrative')), simulated_sources=tuple((segment.key for segment in selected if segment.epistemic is not None and segment.epistemic.classification == 'simulated')))
        full = ContextProjection(id=projection_id, namespace_id=ctx.namespace_id, session_id=command.session_id, consumer_id=command.consumer_id, instance_id=command.instance_id, usage=command.usage, source_watermark=tx.commit_seq, segments=tuple(selected), epistemic_view=epistemic, cursor=cursor, config_digest=config_digest, recorded_at=tx.recorded_at, commit_seq=tx.commit_seq, steward_call_id=steward_call_id, steward_decision=steward_decision, degradations=tuple(sorted(set(degradations))))
        response = ManagedContextResponse(kind='append' if append else 'reset', cursor=cursor, projection=full.model_copy(update={'segments': delta}))
        preparation = ContextPreparation(id=str(uuid4()), state='prepared', request=command, response=response)
        value = {'preparation': preparation.model_dump(mode='json'), 'full_projection': full.model_dump(mode='json'), 'source_scope': scope, 'source_constraints': selected_ctx.source_constraints, 'caller_id': ctx.caller_id}
        await tx.connection.execute(preparations.insert().values(id=preparation.id, namespace_id=ctx.namespace_id, track_id=track_id, record=value))
        return tx.result(preparation)

async def record_segment_in(self: ManagedContexts, view: ReadView | WriteTx, ctx: CallContext, record: CognitiveRecord) -> ContextSegment:
    record = await self.cognition.project_record_in(view, ctx, record)
    if record.body is None:
        raise DomainError('source_purged', 'Purged cognitive content cannot enter a context')
    if isinstance(record.body, (MemoryBody, MemorySemanticsBody, AssociationBody)):
        epistemic = record.body.epistemic
    elif isinstance(record.body, JournalBody):
        epistemic = record.body.perspective
    elif isinstance(record.body, NarrativeThreadBody):
        epistemic = EpistemicStance(classification='narrative', attribution=record.reference.exact())
    else:
        epistemic = None
    segment = ContextSegment(key=record.reference.exact(), text=canonical_json(record.body) if isinstance(record.body, EntityBody) else body_text(record.body), sources=(CognitiveTarget(reference=record.reference),), dependencies=tuple(dict.fromkeys((CognitiveTarget(reference=edge.reference) for edge in (*record.evidence, *tuple(evidence_in(record.body))) if edge.reference is not None))), authority='recorded_cognition', provenance={'source_roots': list(record.source_roots), 'revision': record.reference.revision}, epistemic=epistemic)
    if isinstance(record.body, (DesireBody, ConcernBody)):
        saved = await self.motivation.latest_motivation_assessment_in(view, ctx, record.reference)
        if saved is not None:
            evidence = tuple((CognitiveTarget(reference=ref) for ref in saved.references))
            segment = segment.model_copy(update={'text': segment.text + '\n' + canonical_json({'motivation_assessment': saved.assessment}), 'sources': tuple(dict.fromkeys((*segment.sources, *evidence))), 'dependencies': tuple(dict.fromkeys((*segment.dependencies, *evidence))), 'provenance': {**segment.provenance, 'motivation_assessment_id': saved.id, 'motivation_assessment_commit_seq': saved.commit_seq}})
    return segment

async def preparation(self: ManagedContexts, ctx: CallContext, preparation_id: str) -> ContextPreparation:
    async with self.authority.read(ctx) as view:
        value = await self.read_preparation_in(view, ctx, preparation_id)
        full = ContextProjection.model_validate(value['full_projection'])
        await self.require_projection_in(view, ctx, full)
        return ContextPreparation.model_validate(value['preparation'])

async def cancel_preparation(self: ManagedContexts, ctx: CallContext, preparation_id: str, operation_id: str) -> ContextPreparation:
    async with self.authority.write(ctx, operation_id, operation_kind='context.cancel_preparation', scope=ctx.namespace_id, payload={'id': preparation_id}) as tx:
        if tx.replayed:
            return ContextPreparation.model_validate(tx.replay_result)
        value = await self.read_preparation_in(tx, ctx, preparation_id)
        preparation = ContextPreparation.model_validate(value['preparation'])
        if preparation.state == 'admitted':
            raise DomainError('context_already_admitted', 'Admitted context cannot be cancelled')
        empty = preparation.response.projection.model_copy(update={'segments': ()})
        result = preparation.model_copy(update={'state': 'cancelled', 'response': preparation.response.model_copy(update={'projection': empty})})
        value['full_projection'] = TypeAdapter(JsonValue).validate_json(empty.model_dump_json())
        value['preparation'] = TypeAdapter(JsonValue).validate_json(result.model_dump_json())
        await tx.connection.execute(update(preparations).where(preparations.c.id == preparation_id).values(record=value))
        return tx.result(result)

async def media_segment_in(self: ManagedContexts, view: ReadView | WriteTx, ctx: CallContext, target: MediaTarget, *, required: bool=False) -> ContextSegment:
    from nous_sdk.contracts.derivation import StructuredRepresentation, TextRepresentation
    await self.require_targets_in(view, ctx, (target,))
    reference = target.reference
    if reference.kind in {'material', 'source_region'}:
        material, region, observation_id = await self.materials.media_source_in(view, ctx, reference)
        text = material['text']
        if text is not None and region is not None:
            from nous.material.text import source_region_text
            text = source_region_text(text, region) if any((value is not None for value in (region.text_start, region.text_end, region.json_pointer, region.line_number))) else None
        provenance = {'media_reference': reference.model_dump(mode='json'), 'source_roots': [observation_id], 'media_type': material['media_type']}
        epistemic = None
    else:
        selected = await self.derivations.selected_in(view, ctx, reference)
        info = await self.derivations.source_in(view, ctx, reference)
        if isinstance(selected.payload, TextRepresentation):
            text = selected.payload.text
        elif isinstance(selected.payload, StructuredRepresentation):
            text = canonical_json(selected.payload.value)
        else:
            text = canonical_json(selected.payload)
        provenance = {'media_reference': reference.model_dump(mode='json'), 'source_roots': list(info.roots), 'source_channel_access': info.access.model_dump(mode='json')}
        epistemic = EpistemicStance(classification='derived', attribution=reference.exact())
    return ContextSegment(key=reference.exact(), text=text or reference.exact(), sources=(target,), required=required, authority='source_material', provenance=TypeAdapter(dict[str, JsonValue]).validate_python(provenance), epistemic=epistemic)
