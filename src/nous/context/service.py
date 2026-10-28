"""Context admission and separate observations of contact, use, and outcome."""

from uuid import uuid4

from nous_wave.contracts import (
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    ContextCommand,
    ContextFeedback,
    DomainError,
)
from sqlalchemy import select

from nous.cognition.projection import body_text
from nous.cognition.service import CognitionService
from nous.material.service import MaterialService
from nous.platform.configuration.service import ConfigurationService
from nous.platform.execution import DurableExecution
from nous.platform.models import ModelService
from nous.platform.persistence import Authority
from nous.retrieval.service import RetrievalService

from .managed import ManagedContexts
from .models import contexts, feedback


class ContextService(ManagedContexts):
    def __init__(
        self,
        authority: Authority,
        cognition: CognitionService,
        *,
        retrieval: RetrievalService | None = None,
        materials: MaterialService | None = None,
        configuration: ConfigurationService | None = None,
        models: ModelService | None = None,
        execution: DurableExecution | None = None,
    ) -> None:
        self.authority, self.cognition = authority, cognition
        self.retrieval, self.materials = retrieval, materials
        self.configuration, self.models, self.execution = configuration, models, execution

    async def admit(self, ctx: CallContext, command: ContextCommand):
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="context.admit",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                return tx.replay_result
            records = [await self.cognition.get_in(tx, ctx, ref) for ref in command.references]
            # UTF-8 bytes give a conservative provider-independent upper bound.
            used = 0
            admitted: list[CognitiveRecord] = []
            for record in records:
                size = len(body_text(record.body).encode("utf-8"))
                if used + size <= command.token_budget:
                    admitted.append(record)
                    used += size
            value = {
                "id": str(uuid4()),
                "kind": command.kind,
                "executor": command.executor,
                "purpose": command.purpose,
                "references": [record.reference.model_dump() for record in admitted],
                "token_upper_bound": used,
                "commit_seq": tx.commit_seq,
            }
            await tx.connection.execute(
                contexts.insert().values(
                    id=value["id"],
                    namespace_id=ctx.namespace_id,
                    subject_id=ctx.require_subject(),
                    record=value,
                )
            )
            return tx.result(value)

    async def read(self, ctx: CallContext, context_id: str):
        async with self.authority.read(ctx) as view:
            value = (
                await view.connection.execute(
                    select(contexts.c.record).where(
                        contexts.c.id == context_id,
                        contexts.c.namespace_id == ctx.namespace_id,
                        contexts.c.subject_id == ctx.subject_id,
                    )
                )
            ).scalar_one_or_none()
            if value is None:
                raise DomainError("context_not_found", "Context is unavailable")
            records = [
                await self.cognition.get_in(view, ctx, CognitiveRef.model_validate(ref))
                for ref in value["references"]
            ]
            return {**value, "objects": [record.model_dump(mode="json") for record in records]}

    async def record_feedback(self, ctx: CallContext, command: ContextFeedback):
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="context.feedback",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                return tx.replay_result
            value = (
                await tx.connection.execute(
                    select(contexts.c.record).where(
                        contexts.c.id == command.context_id,
                        contexts.c.namespace_id == ctx.namespace_id,
                        contexts.c.subject_id == ctx.subject_id,
                    )
                )
            ).scalar_one_or_none()
            if value is None or command.reference.model_dump() not in value["references"]:
                raise DomainError(
                    "context_member_not_found", "Feedback target was not admitted to this context"
                )
            await self.cognition.get_in(tx, ctx, command.reference)
            result = {
                "id": str(uuid4()),
                **command.model_dump(mode="json"),
                "commit_seq": tx.commit_seq,
            }
            await tx.connection.execute(
                feedback.insert().values(
                    id=result["id"], context_id=command.context_id, record=result
                )
            )
            return tx.result(result)
