"""Context admission owned behavior."""
from __future__ import annotations
from typing import TYPE_CHECKING
from uuid import uuid4
from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.context import CognitiveTarget, ContextAdmission, ContextAdmissionCommand, ContextPreparation, ContextProjection, ContextReport, ContextReportResult
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select, update
from .feedback_events import record_in
from .models import contexts, preparations, reports, sessions, tracks
from .selection import segment_eligible_in
from .uses import record_uses_in
if TYPE_CHECKING:
    from .managed import ManagedContexts

async def admit_managed(self: ManagedContexts, ctx: CallContext, command: ContextAdmissionCommand) -> ContextAdmission:
    async with self.authority.write(ctx, command.operation_id, operation_kind='context.admit_managed', scope=ctx.namespace_id, payload=command) as tx:
        if tx.replayed:
            replay = ContextAdmission.model_validate(tx.replay_result)
            await self.require_projection_in(tx, ctx, await self.stored_projection_in(tx, ctx, replay.projection_id))
            return replay
        value = await self.read_preparation_in(tx, ctx, command.preparation_id)
        preparation = ContextPreparation.model_validate(value['preparation'])
        if preparation.state not in {'prepared', 'admitted'}:
            raise DomainError('context_not_prepared', 'Only a pending preparation can be admitted')
        projection = ContextProjection.model_validate(value['full_projection'])
        expected = tuple((segment.key for segment in projection.segments))
        if command.applied_segment_ids != expected:
            raise DomainError('context_input_mismatch', 'Applied segment identities do not match the prepared complete input')
        targets = tuple(dict.fromkeys((*[target for segment in projection.segments for target in segment.sources], *command.host_sources)))
        await self.require_targets_in(tx, ctx, command.host_sources)
        await self.require_projection_in(tx, ctx, projection)
        for segment in projection.segments:
            if not await segment_eligible_in(self, tx, ctx, segment):
                raise DomainError('context_selection_inactive', 'An automatically selected source is inactive; prepare this input again')
        accepted_scope = TypeAdapter(tuple[str, ...] | None).validate_python(value['source_scope'])
        accepted_constraints = TypeAdapter(tuple[tuple[str, ...], ...]).validate_python(value.get('source_constraints', ()))
        await self.require_projection_in(tx, ctx.model_copy(update={'allowed_source_ids': accepted_scope, 'source_constraints': accepted_constraints}), projection)
        prior = (await tx.connection.execute(select(tracks.c.record).where(tracks.c.id == projection.cursor.track_id))).scalar_one_or_none()
        if preparation.state == 'prepared' and preparation.response.kind == 'append':
            current = ContextProjection.model_validate(prior) if prior is not None else None
            if current is None or current.cursor != preparation.request.known_cursor:
                raise DomainError('context_cursor_conflict', 'The applied context cursor no longer matches its admitted track')
        result = ContextAdmission(id=str(uuid4()), projection_id=projection.id, namespace_id=ctx.namespace_id, session_id=projection.session_id, consumer_id=projection.consumer_id, instance_id=projection.instance_id, invocation_id=command.invocation_id, rendered_digest=command.rendered_digest, history_generation=command.history_generation, segments=expected, sources=targets, measurement=command.measurement, recorded_at=tx.recorded_at, commit_seq=tx.commit_seq)
        references = [target.reference.model_dump(mode='json') for target in targets if isinstance(target, CognitiveTarget)]
        record = {'id': result.id, 'kind': 'managed', 'executor': projection.consumer_id, 'purpose': projection.usage.purpose, 'references': references, 'measurement': command.measurement.model_dump(mode='json'), 'selected_text_bytes': sum((len(segment.text.encode('utf-8')) for segment in projection.segments)), 'budget_estimate': 'host_measured', 'subject_view': projection.usage.view == 'subject', 'commit_seq': tx.commit_seq, 'managed_admission': result.model_dump(mode='json'), 'caller_id': ctx.caller_id}
        await tx.connection.execute(contexts.insert().values(id=result.id, namespace_id=ctx.namespace_id, record=record))
        if preparation.state == 'admitted':
            pass
        elif prior is None:
            await tx.connection.execute(tracks.insert().values(id=projection.cursor.track_id, namespace_id=ctx.namespace_id, record=projection.model_dump(mode='json')))
        else:
            await tx.connection.execute(update(tracks).where(tracks.c.id == projection.cursor.track_id).values(record=projection.model_dump(mode='json')))
        value['preparation'] = TypeAdapter(JsonValue).validate_json(preparation.model_copy(update={'state': 'admitted'}).model_dump_json())
        await tx.connection.execute(update(preparations).where(preparations.c.id == preparation.id).values(record=value))
        from .lifecycle import session_key
        current_session = await self.session_in(tx, ctx, projection.session_id)
        revised_session = current_session.model_copy(update={'history_generation': command.history_generation, 'history_invalidated': False, 'revision': current_session.revision + 1, 'commit_seq': tx.commit_seq})
        await tx.connection.execute(update(sessions).where(sessions.c.id == session_key(ctx, projection.session_id)).values(record=revised_session.model_dump(mode='json')))
        for target in targets:
            await record_in(self, tx, ctx, context_id=result.id, event_id='admitted:' + str(len(str(target))) + ':' + target.model_dump_json(), target=target, kind='input_admission', occurred_at=tx.recorded_at, invocation_id=result.invocation_id, session_id=result.session_id)
        return tx.result(result)

async def report_managed(self: ManagedContexts, ctx: CallContext, command: ContextReport) -> ContextReportResult:
    async with self.authority.write(ctx, command.operation_id, operation_kind='context.report', scope=ctx.namespace_id, payload=command) as tx:
        if tx.replayed:
            return ContextReportResult.model_validate(tx.replay_result)
        value = (await tx.connection.execute(select(contexts.c.record).where(contexts.c.id == command.admission_id, contexts.c.namespace_id == ctx.namespace_id))).scalar_one_or_none()
        if value is None or 'managed_admission' not in value or value.get('caller_id') != ctx.caller_id:
            raise DomainError('context_admission_not_found', 'Managed context admission is unavailable')
        admission = ContextAdmission.model_validate(value['managed_admission'])
        if admission.invocation_id != command.invocation_id:
            raise DomainError('context_invocation_mismatch', 'Outcome belongs to a different actual invocation')
        if command.output is not None and command.output.raw_source is not None:
            await self.materials.media_source_in(tx, ctx, command.output.raw_source)
        if command.output is not None and command.output.native_model_call_id is not None:
            actual = await self.models.ledger.successful_in(tx, ctx, command.output.native_model_call_id)
            routing = actual.request.get('routing')
            if not isinstance(routing, dict) or routing.get('intent_id') != 'context:' + admission.invocation_id:
                raise DomainError('context_invocation_mismatch', 'The retained model call belongs to another admitted invocation')
            role = actual.request.get('budget_role')
            bound = await self.models.ledger.attempt(ctx, role, 'context:' + admission.invocation_id) if isinstance(role, str) else None
            if bound is None or bound.call_id != actual.id:
                raise DomainError('context_invocation_mismatch', "The native model call is not bound to this host's invocation")
        await record_uses_in(self, tx, ctx, admission, command.uses, command.occurred_at)
        if command.outcome == 'completed':
            for target in admission.sources:
                await record_in(self, tx, ctx, context_id=admission.id, event_id='model-contact:' + target.model_dump_json(), target=target, kind='model_contact', occurred_at=command.occurred_at.isoformat(), invocation_id=admission.invocation_id, session_id=admission.session_id)
        result = ContextReportResult(id=str(uuid4()), admission_id=admission.id, invocation_id=command.invocation_id, outcome=command.outcome, occurred_at=command.occurred_at, uses=command.uses, output=command.output, commit_seq=tx.commit_seq)
        await tx.connection.execute(reports.insert().values(id=result.id, namespace_id=ctx.namespace_id, admission_id=admission.id, record=result.model_dump(mode='json')))
        return tx.result(result)

async def read_admission(self: ManagedContexts, ctx: CallContext, identity: str) -> ContextAdmission:
    async with self.authority.read(ctx) as view:
        value = await view.connection.scalar(select(contexts.c.record).where(contexts.c.id == identity, contexts.c.namespace_id == ctx.namespace_id))
        if value is None or value.get('caller_id') != ctx.caller_id or 'managed_admission' not in value:
            raise DomainError('context_admission_not_found', 'Managed context admission is unavailable')
        admission = ContextAdmission.model_validate(value['managed_admission'])
        await self.require_projection_in(view, ctx, await self.stored_projection_in(view, ctx, admission.projection_id))
        return admission
