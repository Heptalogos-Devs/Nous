"""A model-requested source read records an actual use of its admitted target."""

from __future__ import annotations

from datetime import datetime
from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.context import (
    CognitiveTarget,
    ContextAdmission,
    ContextReportResult,
    ContextSourceReceipt,
    ContextSourceResult,
    ContextUse,
    ReadContextSource,
)
from nous_sdk.contracts.source_graph import ExpandSources
from pydantic import TypeAdapter
from sqlalchemy import select

from nous.platform.persistence import ReadView, WriteTx

from .models import contexts, preparations, reports
from .uses import record_uses_in

if TYPE_CHECKING:
    from .managed import ManagedContexts


async def tool_admission_in(
    self: ManagedContexts,
    view: ReadView | WriteTx,
    ctx: CallContext,
    command: ReadContextSource,
) -> tuple[ContextAdmission, CallContext]:
    raw = await view.connection.scalar(
        select(contexts.c.record).where(
            contexts.c.id == command.admission.admission_id,
            contexts.c.namespace_id == ctx.namespace_id,
            contexts.c.subject_id == ctx.require_subject(),
        )
    )
    if raw is None or raw.get("caller_id") != ctx.caller_id or "managed_admission" not in raw:
        raise DomainError(
            "context_admission_not_found", "The actual model admission is unavailable"
        )
    admission = ContextAdmission.model_validate(raw["managed_admission"])
    target = CognitiveTarget(reference=command.reference)
    if (
        admission.invocation_id != command.admission.invocation_id
        or target not in admission.sources
    ):
        raise DomainError(
            "context_use_not_admitted", "This exact source was not in that model input"
        )
    values = (
        await view.connection.execute(
            select(reports.c.record).where(
                reports.c.admission_id == admission.id,
                reports.c.namespace_id == ctx.namespace_id,
                reports.c.subject_id == ctx.subject_id,
            )
        )
    ).scalars()
    if not any(
        result.invocation_id == admission.invocation_id
        and result.outcome == "completed"
        and result.output is not None
        and command.tool_call_id in result.output.tool_call_ids
        for result in (ContextReportResult.model_validate(value) for value in values)
    ):
        raise DomainError(
            "context_tool_call_unverified", "The completed model output has no such tool call"
        )
    prepared = await view.connection.scalar(
        select(preparations.c.record).where(
            preparations.c.namespace_id == ctx.namespace_id,
            preparations.c.subject_id == ctx.subject_id,
            preparations.c.record["full_projection"]["id"].as_string() == admission.projection_id,
        )
    )
    if prepared is None or prepared.get("caller_id") != ctx.caller_id:
        raise DomainError(
            "context_preparation_unavailable",
            "The model input's original source authorization is unavailable",
        )
    accepted = ctx.model_copy(
        update={
            "allowed_source_ids": TypeAdapter(tuple[str, ...] | None).validate_python(
                prepared["source_scope"]
            ),
            "source_constraints": TypeAdapter(tuple[tuple[str, ...], ...]).validate_python(
                prepared["source_constraints"]
            ),
        }
    )
    effective = ctx.model_copy(
        update={
            "source_constraints": tuple(
                dict.fromkeys((*ctx.source_groups(), *accepted.source_groups()))
            ),
            "allowed_source_ids": None,
        }
    )
    await self.require_targets_in(view, effective, (target,))
    return admission, effective


async def read_context_source(
    self: ManagedContexts,
    ctx: CallContext,
    command: ReadContextSource,
) -> ContextSourceResult:
    if self.retrieval is None:
        raise DomainError("source_reader_unavailable", "The source graph owner is unavailable")
    async with self.authority.read(ctx) as view:
        _, effective = await tool_admission_in(self, view, ctx, command)
        memory = await self.cognition.get_in(view, effective, command.reference)
        if memory.purged or memory.body is None:
            raise DomainError("source_purged", "The admitted cognitive source has been purged")
    graph = await self.retrieval.expand_sources(
        effective,
        ExpandSources(
            targets=(CognitiveTarget(reference=command.reference),),
        ),
    )
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="context.source_read",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        admission, effective = await tool_admission_in(self, tx, ctx, command)
        if tx.replayed:
            receipt = ContextSourceReceipt.model_validate(tx.replay_result)
        else:
            await record_uses_in(
                self,
                tx,
                effective,
                admission,
                (
                    ContextUse(
                        event_id="tool:" + command.tool_call_id + ":" + command.reference.exact(),
                        target=CognitiveTarget(reference=command.reference),
                        kind="referenced",
                    ),
                ),
                datetime.fromisoformat(tx.recorded_at),
            )
            receipt = tx.result(
                ContextSourceReceipt(
                    admission=command.admission,
                    tool_call_id=command.tool_call_id,
                    reference=command.reference,
                    commit_seq=tx.commit_seq,
                )
            )
    return ContextSourceResult(memory=memory, sources=graph, use=receipt)
