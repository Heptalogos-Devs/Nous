"""A native host executes the same complete request directory that it admitted."""
from __future__ import annotations
from datetime import UTC, datetime
from hashlib import sha256
from typing import TYPE_CHECKING
from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.context import CognitiveTarget, ContextInvocationOutput, ContextModelCall, ContextModelPlan, ContextModelPreview, ContextModelResult, ContextProjection, ContextReport, ContextReportResult
from nous_sdk.contracts.model_roles import InvocationSnapshot
from nous_sdk.contracts.prompt_compilation import PromptMessage
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select
from nous.platform.models.generation import MediaInput
from nous.platform.models.routing import GenerationPlan
from nous.platform.persistence.authority import canonical_json
from .models import reports
if TYPE_CHECKING:
    from .managed import ManagedContexts

async def accepted_plan(self: ManagedContexts, ctx: CallContext, command: ContextModelPreview) -> tuple[ContextModelPlan, GenerationPlan, tuple[MediaInput, ...],tuple[PromptMessage,...]]:
    async with self.authority.read(ctx) as view:
        value = await self.read_preparation_in(view, ctx, command.preparation_id)
        projection = ContextProjection.model_validate(value['full_projection'])
        sources = tuple((target for segment in projection.segments for target in segment.sources))
        await self.require_projection_in(view, ctx, projection)
        source_ids: set[str] = set()
        for target in sources:
            if isinstance(target, CognitiveTarget):
                record = await self.cognition.get_in(view, ctx, target.reference)
                source_ids.update(record.source_roots)
            else:
                info = await self.materials.provenance_in(view, ctx, target.reference)
                source_ids.update(info.material_ids)
                source_ids.update(info.roots)
    media: list[MediaInput] = []
    for target in sources:
        if target.kind == 'media':
            if self.models.preview_media_resolver is None:
                raise DomainError('media_unavailable', 'The context media resolver is unavailable')
            media.append(await self.models.preview_media_resolver(ctx, target.reference))
    context_messages=tuple(PromptMessage(role='system' if segment.required and segment.provenance.get('supply')=='current_personality' else 'user',content=segment.text) for segment in projection.segments)
    plan = await self.models.accept_generation(ctx, command.role, dict(command.input), source_ids=tuple(sorted(source_ids)), invocation_id='context:' + command.invocation_id, media=tuple(media),context_messages=context_messages)
    requests = tuple((InvocationSnapshot.model_validate(route.invocation) for route in plan.routes if route.invocation is not None))
    if not requests:
        raise DomainError('model_plan_unavailable', 'The complete accepted provider request directory is unavailable')
    return (ContextModelPlan(preparation_id=command.preparation_id, invocation_id=command.invocation_id, role=command.role, requests=requests), plan, tuple(media), context_messages)

async def preview_model(self: ManagedContexts, ctx: CallContext, command: ContextModelPreview) -> ContextModelPlan:
    public, _, _, _ = await accepted_plan(self, ctx, command)
    return public

async def invoke_model(self: ManagedContexts, ctx: CallContext, command: ContextModelCall) -> ContextModelResult:
    admission = await self.read_admission(ctx, command.admission_id)
    public, plan, media, context_messages = await accepted_plan(self, ctx, command)
    if admission.invocation_id != command.invocation_id or admission.rendered_digest != sha256(canonical_json(public).encode()).hexdigest():
        raise DomainError('context_input_mismatch', 'This provider request differs from the complete host input admission')
    definition = self.models.roles.resolve(plan.contract_purpose or plan.purpose, plan.variant)
    generated = await self.models.generate(ctx, command.role, dict(command.input), definition.output_type, source_ids=plan.source_ids, invocation_id='context:' + command.invocation_id, cache_policy='invocation', media=media,context_messages=context_messages)
    async with self.authority.read(ctx) as view:
        old = await view.connection.scalar(select(reports.c.record).where(reports.c.namespace_id == ctx.namespace_id, reports.c.admission_id == admission.id, reports.c.record['output']['native_model_call_id'].as_string() == generated.call_id))
    report = ContextReportResult.model_validate(old) if old is not None else await self.report_managed(ctx, ContextReport(operation_id=command.operation_id, admission_id=admission.id, invocation_id=command.invocation_id, outcome='completed', occurred_at=datetime.now(UTC), output=ContextInvocationOutput(model=plan.routes[0].route.model, native_model_call_id=generated.call_id, status='succeeded')))
    return ContextModelResult(call_id=generated.call_id, output=TypeAdapter(JsonValue).validate_json(TypeAdapter(object).dump_json(generated.output)), report=report)
