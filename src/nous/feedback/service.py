"""One owner freezes real feedback, schedules it, and delegates changes to CorrectionService."""

from __future__ import annotations

from nous_sdk.presentation import body_text

from datetime import UTC, datetime
from typing import TYPE_CHECKING
from uuid import uuid4

from nous_sdk.contracts import (
    CallContext,
    CognitiveRef,
    DomainError,
    MemoryBody,
    TimePoint,
)
from nous_sdk.contracts.common import utc_now
from nous_sdk.contracts.corrections import ApplyCorrection, PlanCorrection, RollbackCorrection
from nous_sdk.contracts.feedback import (
    EnqueueFeedback,
    FeedbackDecision,
    FeedbackInput,
    FeedbackPage,
    FeedbackPolicy,
    FeedbackQuery,
    FeedbackTarget,
    FeedbackTask,
    RetryFeedback,
    RollbackFeedback,
)
from nous_sdk.contracts.records import RecordsQuery
from pydantic import JsonValue
from sqlalchemy import func, select, update

from nous.identity.access import require_namespace_in
from nous.material.service import observations
from nous.platform.errors import NousError
from nous.platform.execution import JobSpec
from nous.platform.persistence import ReadView, WriteTx

from .schema import tasks

if TYPE_CHECKING:
    from nous.cognition.corrections import CorrectionService
    from nous.cognition.records import CognitiveRecords
    from nous.platform.configuration.service import ConfigurationService
    from nous.platform.execution import DurableExecution
    from nous.platform.models import ModelService
    from nous.platform.persistence import Authority


class FeedbackService:
    def __init__(
        self,
        authority: Authority,
        cognition: CognitiveRecords,
        configuration: ConfigurationService,
        execution: DurableExecution,
        models: ModelService,
        corrections: CorrectionService,
    ) -> None:
        self.authority = authority
        self.cognition = cognition
        self.configuration = configuration
        self.execution = execution
        self.models = models
        self.corrections = corrections

    async def policy(self, ctx: CallContext) -> FeedbackPolicy:
        return FeedbackPolicy.model_validate(
            (await self.configuration.effective(ctx, "feedback"))["value"]
        )

    async def get_in(
        self, view: ReadView | WriteTx, ctx: CallContext, task_id: str
    ) -> FeedbackTask:
        await require_namespace_in(view, ctx)
        raw = await view.connection.scalar(
            select(tasks.c.record).where(
                tasks.c.id == task_id,
                tasks.c.namespace_id == ctx.namespace_id,
            )
        )
        if raw is None:
            raise DomainError("feedback_not_found", "Feedback task is unavailable")
        task = FeedbackTask.model_validate(raw)
        if task.state != "unavailable":
            for ref in (*task.references, *task.supports):
                await self.cognition.get_in(view, ctx, ref)
        return task

    async def get(self, ctx: CallContext, task_id: str) -> FeedbackTask:
        async with self.authority.read(ctx) as view:
            return await self.get_in(view, ctx, task_id)

    async def list(self, ctx: CallContext, query: FeedbackQuery) -> FeedbackPage:
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            statement = select(tasks.c.id).where(
                tasks.c.namespace_id == ctx.namespace_id,
            )
            if query.cursor:
                statement = statement.where(tasks.c.id > query.cursor)
            if query.states:
                statement = statement.where(tasks.c.record["state"].as_string().in_(query.states))
            if query.session_id is not None:
                statement = statement.where(
                    tasks.c.record["session_id"].as_string() == query.session_id
                )
            ids = list(
                (
                    await view.connection.scalars(
                        statement.order_by(tasks.c.id).limit(query.limit + 1)
                    )
                ).all()
            )
            items = tuple([await self.get_in(view, ctx, value) for value in ids[: query.limit]])
            return FeedbackPage(
                items=items,
                next_cursor=str(ids[query.limit - 1]) if len(ids) > query.limit else None,
            )

    async def enqueue(self, ctx: CallContext, command: EnqueueFeedback) -> FeedbackTask:
        policy = await self.policy(ctx)
        when = command.queried_at or utc_now()
        if when.domain != "unix_ms":
            raise DomainError("feedback_time_invalid", "Feedback windows require Unix time")
        async with self.authority.write(ctx) as tx:
            await require_namespace_in(tx, ctx)
            for reference in command.references:
                await self.cognition.get_in(tx, ctx, reference)
            due = when.coordinate + round(policy.window_hours * 3600000)
            task = FeedbackTask(
                id=str(uuid4()),
                namespace_id=ctx.namespace_id,
                caller_id=ctx.caller_id,
                session_id=command.session_id,
                query=command.query,
                references=tuple(dict.fromkeys(command.references)),
                queried_at=when,
                due_at=TimePoint(coordinate=due),
                policy=policy,
                state="pending" if policy.enabled else "disabled",
                commit_seq=tx.commit_seq,
                created_at=tx.recorded_at,
                updated_at=tx.recorded_at,
            )
            if policy.enabled:
                job = await self.execution.enqueue_in(
                    tx,
                    JobSpec('nous.feedback.evaluate', 'default', args=(ctx.model_dump(mode='json'), task.id), workflow_id='feedback:' + task.id, delay_seconds=max(0, (due - utc_now().coordinate) / 1000), scope=ctx.namespace_id),
                )
                task = task.model_copy(update={"job_id": job.workflow_id})
            await tx.connection.execute(
                tasks.insert().values(
                    id=task.id,
                    namespace_id=ctx.namespace_id,
                    record=task.model_dump(mode="json"),
                )
            )
            return task

    async def _save(
        self, ctx: CallContext, task_id: str, phase: str, **changes: object
    ) -> FeedbackTask:
        async with self.authority.write(ctx) as tx:
            task = await self.get_in(tx, ctx, task_id)
            if task.state == "unavailable":
                raise DomainError("source_unavailable", "Feedback sources are unavailable")
            value = FeedbackTask.model_validate(
                {**task.model_dump(), **changes, "commit_seq": tx.commit_seq, "updated_at": tx.recorded_at}
            )
            await tx.connection.execute(
                update(tasks)
                .where(tasks.c.id == task_id)
                .values(record=value.model_dump(mode="json"))
            )
            return value

    async def _freeze(self, ctx: CallContext, task: FeedbackTask) -> FeedbackTask:
        if task.input is not None:
            return task
        async with self.authority.read(ctx) as view:
            current = (
                await self.cognition.records_in(
                    view,
                    ctx,
                    RecordsQuery(object_ids=tuple(ref.id for ref in task.references), limit=100),
                )
            ).items
            by_id = {entry.record.reference.id: entry.record for entry in current}
            if any(
                by_id.get(ref.id) is None or by_id[ref.id].reference != ref
                for ref in task.references
            ):
                raise DomainError(
                    "feedback_revision_conflict",
                    "A supplied cognitive revision changed before feedback evaluation",
                )
            start = (
                datetime.fromtimestamp(task.queried_at.coordinate / 1000).astimezone().isoformat()
            )
            end = datetime.fromtimestamp(task.due_at.coordinate / 1000).astimezone().isoformat()
            events = list(
                (
                    await view.connection.scalars(
                        select(observations.c.id)
                        .where(
                            observations.c.namespace_id == ctx.namespace_id,
                            observations.c.kind == "contact",
                            observations.c.active.is_(True),
                            observations.c.record["origin"]["conversation_ref"].as_string()
                            == task.session_id,
                            # Authority recording time bounds subsequent actual contacts; imports cannot enter.
                            func.julianday(observations.c.record["recorded_at"].as_string())
                            > func.julianday(start),
                            func.julianday(observations.c.record["recorded_at"].as_string())
                            <= func.julianday(end),
                        )
                        .order_by(observations.c.commit_seq.desc())
                        .limit(task.policy.max_messages)
                    )
                ).all()
            )
            evidence = (
                (
                    await self.cognition.records_in(
                        view,
                        ctx,
                        RecordsQuery(kinds=("memory",), source_ids=tuple(events), limit=1000),
                    )
                ).items
                if events
                else ()
            )
            messages = tuple(
                entry.record
                for entry in reversed(evidence)
                if isinstance(entry.record.body, MemoryBody)
                and entry.record.body.contact_observation_id in events
                and body_text(entry.record.body).strip()
            )
            frozen = FeedbackInput(
                query=task.query,
                targets=tuple(
                    FeedbackTarget(key=f"t{i}", record=by_id[ref.id])
                    for i, ref in enumerate(task.references)
                ),
                messages=tuple(
                    FeedbackTarget(key=f"e{i}", record=row) for i, row in enumerate(messages)
                ),
            )
        return await self._save(
            ctx,
            task.id,
            "freeze",
            input=frozen,
            supports=tuple(row.record.reference for row in frozen.messages),
            state="pending" if frozen.messages else "no_feedback",
        )

    async def run(self, context: dict[str, object], task_id: str) -> dict[str, object]:
        ctx = CallContext.model_validate(context).model_copy(update={"clock": utc_now()})
        try:
            return await self.evaluate(ctx, task_id)
        except (DomainError, NousError) as error:
            task = await self.get(ctx, task_id)
            if task.state != "unavailable":
                await self._save(ctx, task_id, "failure:" + str(task.job_id), state="failed",
                    error={"code": error.code, "message": str(error)})
            raise

    async def retry(self, ctx: CallContext, command: RetryFeedback) -> FeedbackTask:
        async with self.authority.write(ctx) as tx:
            task = await self.get_in(tx, ctx, command.task_id)
            if task.state != "failed":
                raise DomainError("feedback_not_failed", "Only failed feedback evaluation can be resumed")
            if not FeedbackPolicy.model_validate((await self.configuration.effective(ctx, "feedback"))["value"]).enabled:
                raise DomainError("feedback_disabled", "Feedback correction is disabled")
            job = await self.execution.enqueue_in(tx, JobSpec('nous.feedback.evaluate', 'default', args=(ctx.model_dump(mode='json'), task.id), scope=ctx.namespace_id))
            updated = task.model_copy(update={"state": "pending", "error": None,
                "job_id": job.workflow_id, "commit_seq": tx.commit_seq})
            await tx.connection.execute(update(tasks).where(tasks.c.id == task.id).values(record=updated.model_dump(mode="json")))
            return updated

    async def evaluate(self, ctx: CallContext, task_id: str) -> dict[str, object]:
        task = await self.get(ctx, task_id)
        if task.state != "pending":
            return task.model_dump(mode="json")
        if not (await self.policy(ctx)).enabled:
            return (await self._save(ctx, task_id, "disabled", state="disabled")).model_dump(
                mode="json"
            )
        task = await self._freeze(ctx, task)
        if task.state == "no_feedback":
            return task.model_dump(mode="json")
        assert task.input is not None
        if task.decision is None:
            generated = await self.models.generate(
                ctx,
                "feedback",
                task.input.model_dump(mode="json"),
                FeedbackDecision,
                source_ids=tuple(
                    sorted(
                        {
                            root
                            for target in (*task.input.targets, *task.input.messages)
                            for root in target.record.source_roots
                        }
                    )
                ),
            )
            decision = generated.output
            keys = {target.key for target in task.input.targets}
            if not set(decision.target_keys).issubset(keys):
                raise DomainError(
                    "feedback_target_invalid",
                    "Feedback selected a target outside its supplied query",
                )
            if not set(decision.message_keys).issubset(message.key for message in task.input.messages):
                raise DomainError("feedback_evidence_invalid", "Feedback selected evidence outside its actual messages")
            task = await self._save(
                ctx, task_id, "classify", decision=decision, producer=generated.producer
            )
        decision = task.decision
        assert decision is not None
        assert task.input is not None
        if decision.decision in {"none", "confirm"} or not decision.target_keys:
            return (await self._save(ctx, task_id, "no-change", state="no_change")).model_dump(
                mode="json"
            )
        if (
            decision.decision == "supplement"
            or decision.confidence < task.policy.auto_apply_threshold
            or not decision.instruction.strip()
            or not decision.message_keys
        ):
            return (await self._save(ctx, task_id, "review", state="review")).model_dump(
                mode="json"
            )
        selected = tuple(
            target.record.reference
            for target in task.input.targets
            if target.key in decision.target_keys
        )
        plan = await self.corrections.plan(
            ctx,
            PlanCorrection(
                operation_id="feedback:" + task.id + ":plan",
                instruction=decision.instruction,
                selection=RecordsQuery(object_ids=tuple(ref.id for ref in selected), limit=100),
                supports=tuple(message.record.reference for message in task.input.messages if message.key in decision.message_keys),
            ),
        )
        if not plan.changes and not plan.questions:
            return (await self._save(ctx, task_id, "no-plan-change", state="no_change", correction_plan_id=plan.id)).model_dump(mode="json")
        if {target.record.reference.exact() for target in plan.targets} != {
            ref.exact() for ref in selected
        } or plan.questions:
            return (
                await self._save(
                    ctx, task_id, "review-plan", state="review", correction_plan_id=plan.id
                )
            ).model_dump(mode="json")
        result = await self.corrections.apply(
            ctx, ApplyCorrection(operation_id="feedback:" + task.id + ":apply", plan_id=plan.id)
        )
        return (
            await self._save(
                ctx,
                task_id,
                "apply",
                correction_plan_id=plan.id,
                result=result,
                state="review" if result.conflicts else "applied",
            )
        ).model_dump(mode="json")

    async def rollback(self, ctx: CallContext, command: RollbackFeedback) -> FeedbackTask:
        task = await self.get(ctx, command.task_id)
        if task.correction_plan_id is None:
            raise DomainError(
                "feedback_not_applied", "This feedback task has no correction to roll back"
            )
        result = await self.corrections.rollback(
            ctx,
            RollbackCorrection(
                operation_id=command.operation_id,
                plan_id=task.correction_plan_id,
                reason=command.reason,
            ),
        )
        return await self._save(
            ctx,
            task.id,
            "rollback:" + command.operation_id,
            result=result,
            state="review" if result.conflicts else "rolled_back",
            rollback_actor=ctx.caller_id,
            rollback_reason=command.reason,
            rollback_recorded_at=datetime.now(UTC).isoformat(),
        )

    async def purge_references_in(
        self, tx: WriteTx, ctx: CallContext, references: tuple[CognitiveRef, ...]
    ) -> None:
        ids = {reference.id for reference in references}
        rows = (
            await tx.connection.execute(
                select(tasks.c.id, tasks.c.record).where(
                    tasks.c.namespace_id == ctx.namespace_id
                )
            )
        ).all()
        for row in rows:
            value = FeedbackTask.model_validate(row.record)
            if ids.intersection(ref.id for ref in (*value.references, *value.supports)):
                cleaned = value.model_copy(
                    update={
                        "query": "",
                        "input": None,
                        "decision": None,
                        "result": None,
                        "rollback_reason": "",
                        "state": "unavailable",
                        "commit_seq": tx.commit_seq,
                    }
                )
                await tx.connection.execute(
                    update(tasks)
                    .where(tasks.c.id == row.id)
                    .values(record=cleaned.model_dump(mode="json"))
                )
                task_id = value.id


def task_mentions(value: JsonValue, task_id: str, references: set[str]) -> bool:
    if isinstance(value, dict):
        identity = value.get("id")
        return (
            value.get("task_id") == task_id
            or (isinstance(identity, str) and identity in references)
            or any(task_mentions(item, task_id, references) for item in value.values())
        )
    if isinstance(value, list):
        return any(task_mentions(item, task_id, references) for item in value)
    return False
