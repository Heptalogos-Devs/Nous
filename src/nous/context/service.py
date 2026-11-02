"""Context admission and separate observations of contact, use, and outcome."""
from __future__ import annotations
from typing import TYPE_CHECKING
from uuid import uuid4
from nous_sdk.contracts import CallContext, CognitiveRecord, CognitiveRef, ContextCommand, ContextFeedback, DomainError
from nous_sdk.contracts.targets import CognitiveTarget
from nous_sdk.presentation import body_text
from sqlalchemy import select
from nous.cognition.motivation_records import MotivationService
from nous.cognition.records import CognitiveRecords
from nous.material.derived import DerivedService
from nous.material.service import MaterialService
from nous.platform.configuration.service import ConfigurationService
from nous.platform.execution import DurableExecution
from nous.platform.models import ModelService
from nous.platform.persistence import Authority
from nous.retrieval.service import RetrievalService
from .feedback_events import record_in
from .managed import ManagedContexts
from .models import contexts
if TYPE_CHECKING:
    from nous.feedback.service import FeedbackService
    from nous.references.reading import ReadingService
    from nous.symbols.service import SymbolsService

class ContextService(ManagedContexts):

    def __init__(self, authority: Authority, cognition: CognitiveRecords, *, retrieval: RetrievalService, materials: MaterialService, configuration: ConfigurationService, models: ModelService, execution: DurableExecution, derivations: DerivedService, feedback_owner: FeedbackService, symbols: SymbolsService, motivation: MotivationService, reading: ReadingService) -> None:
        self.reading = reading
        self.motivation = motivation
        self.authority, self.cognition = (authority, cognition)
        self.retrieval, self.materials = (retrieval, materials)
        self.configuration, self.models, self.execution = (configuration, models, execution)
        self.derivations = derivations
        self.feedback_owner = feedback_owner
        self.symbols = symbols

    async def admit(self, ctx: CallContext, command: ContextCommand):
        async with self.authority.write(ctx, command.operation_id, operation_kind='context.admit', scope=ctx.namespace_id, payload=command) as tx:
            if tx.replayed:
                for reference in command.references:
                    await self.cognition.get_in(tx, ctx, reference)
                return tx.replay_result
            records = [await self.cognition.get_in(tx, ctx, ref) for ref in command.references]
            used = 0
            admitted: list[CognitiveRecord] = []
            for record in records:
                size = len(body_text(record.body).encode('utf-8'))
                if used + size <= command.token_budget:
                    admitted.append(record)
                    used += size
            value = {'id': str(uuid4()), 'kind': command.kind, 'executor': command.executor, 'purpose': command.purpose, 'references': [record.reference.model_dump() for record in admitted], 'selected_text_bytes': used, 'budget_estimate': 'utf8_bytes', 'session_id': command.session_id, 'caller_id': ctx.caller_id, 'commit_seq': tx.commit_seq}
            await tx.connection.execute(contexts.insert().values(id=value['id'], namespace_id=ctx.namespace_id, record=value))
            return tx.result(value)

    async def read(self, ctx: CallContext, context_id: str):
        async with self.authority.read(ctx) as view:
            value = (await view.connection.execute(select(contexts.c.record).where(contexts.c.id == context_id, contexts.c.namespace_id == ctx.namespace_id))).scalar_one_or_none()
            if value is None:
                raise DomainError('context_not_found', 'Context is unavailable')
            records = [await self.cognition.get_in(view, ctx, CognitiveRef.model_validate(ref)) for ref in value['references']]
            return {**{key: value[key] for key in ('id', 'kind', 'executor', 'purpose', 'references', 'commit_seq')}, 'selected_text_bytes': value.get('selected_text_bytes', value.get('token_upper_bound', 0)), 'budget_estimate': value.get('budget_estimate', 'legacy_utf8_bytes'), 'measurement': value.get('managed_admission', {}).get('measurement'), 'objects': [record.model_dump(mode='json') for record in records]}

    async def record_feedback(self, ctx: CallContext, command: ContextFeedback):
        async with self.authority.write(ctx, command.operation_id, operation_kind='context.feedback', scope=ctx.namespace_id, payload=command) as tx:
            if tx.replayed:
                return tx.replay_result
            value = (await tx.connection.execute(select(contexts.c.record).where(contexts.c.id == command.context_id, contexts.c.namespace_id == ctx.namespace_id))).scalar_one_or_none()
            if value is None or command.reference.model_dump() not in value['references']:
                raise DomainError('context_member_not_found', 'Feedback target was not admitted to this context')
            if 'managed_admission' in value and value.get('caller_id') != ctx.caller_id:
                raise DomainError('context_admission_not_found', 'Managed admission belongs to its authenticated host')
            await self.cognition.get_in(tx, ctx, command.reference)
            managed = value.get('managed_admission', {})
            result = await record_in(self, tx, ctx, context_id=command.context_id, event_id=command.operation_id, target=CognitiveTarget(reference=command.reference), kind=command.kind, description=command.description, outcome=command.outcome, occurred_at=tx.recorded_at, session_id=managed.get('session_id') or value.get('session_id'), invocation_id=managed.get('invocation_id'), subject_view=value.get('subject_view', value.get('executor') == 'subject'))
            return tx.result(result)
