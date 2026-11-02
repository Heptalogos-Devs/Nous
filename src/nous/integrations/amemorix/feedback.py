"""A-Memorix tool identities translate to native feedback queries and corrections."""

from __future__ import annotations

from datetime import UTC, datetime
from hashlib import sha256
from typing import TYPE_CHECKING, Literal

from nous_sdk.contracts import CallContext, CognitiveRef, Contract, DomainError
from nous_sdk.contracts.feedback import EnqueueFeedback, FeedbackTask, RollbackFeedback
from nous_sdk.contracts.targets import CognitiveTarget
from pydantic import Field, JsonValue
from sqlalchemy import Column, Integer, String, Table, UniqueConstraint, select

from nous.platform.persistence import metadata
from nous.platform.persistence.authority import canonical_json

from .helpers import json_object, point

if TYPE_CHECKING:
    from .service import AMemorixService


bindings = Table(
    "amemorix_feedback_bindings",
    metadata,
    Column("id", Integer, primary_key=True, autoincrement=True),
    Column("namespace_id", String, nullable=False),
    
    Column("caller_id", String, nullable=False),
    Column("query_tool_id", String, nullable=False),
    Column("session_id", String, nullable=False),
    Column("task_id", String, nullable=False),
    UniqueConstraint('namespace_id', "caller_id", "query_tool_id", "session_id"),
)


class EnqueueInput(Contract):
    query_tool_id: str = Field(min_length=1)
    session_id: str = Field(min_length=1)
    query_timestamp: str | float | None = None
    structured_content: dict[str, JsonValue] | None = None


class AdminInput(Contract):
    action: Literal["get", "list", "rollback"]
    task_id: int | None = None
    status: str | tuple[str, ...] | None = None
    statuses: tuple[str, ...] = ()
    rollback_status: str | tuple[str, ...] | None = None
    rollback_statuses: tuple[str, ...] = ()
    query: str = ""
    limit: int = Field(default=50, ge=1, le=200)
    requested_by: str = ""
    reason: str = "Rollback feedback correction"
    operation_id: str | None = None


async def enqueue(
    self: AMemorixService, ctx: CallContext, arguments: dict[str, JsonValue]
) -> dict[str, JsonValue]:
    if not (await self.app.feedback.policy(ctx)).enabled:
        return {"success": False, "queued": False, "reason": "feedback_correction_disabled"}
    if not all(isinstance(arguments.get(key), str) and str(arguments[key]).strip() for key in ("query_tool_id", "session_id")):
        return {"success": False, "queued": False, "reason": "missing_required_fields"}
    command = EnqueueInput.model_validate(arguments)
    content = command.structured_content or {}
    hits = content.get("hits", [])
    refs: list[CognitiveRef] = []
    if isinstance(hits, list):
        for hit in hits:
            if not isinstance(hit, dict):
                continue
            raw_metadata = hit.get("metadata")
            raw = raw_metadata.get("reference") if isinstance(raw_metadata, dict) else None
            if raw is not None:
                reference = CognitiveRef.model_validate(raw)
                await self.app.cognition.get(ctx, reference)
                refs.append(reference)
            elif isinstance(hit.get("hash"), str):
                resolved = await self.app.references.resolve(ctx, str(hit["hash"]))
                if isinstance(resolved.reference, CognitiveTarget):
                    refs.append(resolved.reference.reference)
    if not refs:
        return {"success": False, "queued": False, "reason": "no_hits"}
    identity = (
        "amemorix-feedback:"
        + sha256(
            canonical_json(
                (
                    ctx.caller_id,
                    ctx.namespace_id,
                    ctx.namespace_id,
                    command.query_tool_id,
                    command.session_id,
                )
            ).encode()
        ).hexdigest()
    )
    task = await self.app.feedback.enqueue(
        ctx,
        EnqueueFeedback(
            operation_id=identity,
            session_id=command.session_id,
            references=tuple(dict.fromkeys(refs)),
            query=str(content.get("query") or ""),
            queried_at=point(command.query_timestamp),
        ),
    )
    async with self.app.authority.write(
        ctx,
        identity + ":bind",
        operation_kind="amemorix.feedback.bind",
        scope=ctx.namespace_id,
        payload={
            "query_tool_id": command.query_tool_id,
            "session_id": command.session_id,
            "task_id": task.id,
        },
    ) as tx:
        if tx.replayed:
            task_id = int(str(tx.replay_result))
        else:
            result = await tx.connection.execute(
                bindings.insert().values(
                    namespace_id=ctx.namespace_id,
                    
                    caller_id=ctx.caller_id,
                    query_tool_id=command.query_tool_id,
                    session_id=command.session_id,
                    task_id=task.id,
                )
            )
            assert result.inserted_primary_key is not None
            task_id = int(result.inserted_primary_key[0])
            tx.result(task_id)
    return json_object(
        {
            "success": True,
            "queued": task.state == "pending",
            "task_id": task_id,
            "native_task_id": task.id,
            "job_id": task.job_id,
            "query_tool_id": command.query_tool_id,
            "due_at": datetime.fromtimestamp(task.due_at.coordinate / 1000, UTC).isoformat(),
            "task": await describe(self, ctx, task, task_id, command.query_tool_id),
        }
    )


async def admin(
    self: AMemorixService, ctx: CallContext, arguments: dict[str, JsonValue]
) -> dict[str, JsonValue]:
    command = AdminInput.model_validate(arguments)
    async with self.app.authority.read(ctx) as view:
        statement = select(bindings).where(
            bindings.c.namespace_id == ctx.namespace_id
        )
        if command.task_id is not None:
            statement = statement.where(bindings.c.id == command.task_id)
        rows = (
            (await view.connection.execute(statement.order_by(bindings.c.id.desc())))
            .mappings()
            .all()
        )
    if command.action in {"get", "rollback"}:
        if command.task_id is None or not rows:
            return {"success": False, "error": "Feedback task is unavailable", "code": "feedback_not_found"}
        row = rows[0]
        task = await self.app.feedback.get(ctx, str(row["task_id"]))
        if command.action == "rollback":
            task = await self.app.feedback.rollback(
                ctx,
                RollbackFeedback(
                    operation_id=command.operation_id or f"amemorix-feedback:{row['id']}:rollback",
                    task_id=task.id,
                    reason=command.reason,
                ),
            )
        return json_object({"success": task.result is None or not task.result.conflicts,
            "task": await describe(self, ctx, task, int(row["id"]), str(row["query_tool_id"]), detail=True)})
    states = set(command.statuses)
    if command.status:
        states.update((command.status,) if isinstance(command.status, str) else command.status)
    rollback = set(command.rollback_statuses)
    if command.rollback_status:
        rollback.update((command.rollback_status,) if isinstance(command.rollback_status, str) else command.rollback_status)
    selected: list[dict[str, JsonValue]] = []
    for row in rows:
        task = await self.app.feedback.get(ctx, str(row["task_id"]))
        summary = await describe(self, ctx, task, int(row["id"]), str(row["query_tool_id"]))
        if (
            states
            and str(summary["task_status"]) not in states
            or rollback
            and str(summary["rollback_status"]) not in rollback
        ):
            continue
        if (
            command.query
            and command.query.casefold() not in (task.query + str(row["query_tool_id"])).casefold()
        ):
            continue
        selected.append(summary)
        if len(selected) >= command.limit:
            break
    return json_object({"success": True, "items": selected, "count": len(selected)})


async def describe(self: AMemorixService, ctx: CallContext, task: FeedbackTask, task_id: int,
    query_tool_id: str, *, detail: bool = False) -> dict[str, JsonValue]:
    status = "applied" if task.state in {"applied", "rolled_back"} else "error" if task.state in {"failed", "unavailable"} else "pending" if task.state == "pending" else "skipped"
    execution_state = None
    if task.job_id and task.caller_id == ctx.caller_id:
        try:
            execution_state = (await self.app.execution.status(ctx, task.job_id)).status
        except DomainError as error:
            if error.code != "job_not_found":
                raise
        if task.state == "pending" and execution_state == "PENDING":
            status = "running"
    rollback_status = "rolled_back" if task.state == "rolled_back" else "error" if task.result and task.result.action == "rollback" and task.result.conflicts else "none"
    if rollback_status == "error":
        status = "applied"
    references = tuple(change.reference for change in task.result.records) if task.result else ()
    affected: dict[str, int] = {}
    for reference in references:
        affected[reference.kind] = affected.get(reference.kind, 0) + 1
    decision = task.decision
    payload: dict[str, JsonValue] = {"decision": decision.decision, "confidence": decision.confidence,
        "reason": decision.reason, "feedback_message_count": len(task.supports),
        "target_hashes": [target.record.reference.id for target in task.input.targets if target.key in decision.target_keys] if task.input else []} if decision else {}
    value = json_object({"id": task_id, "task_id": task_id, "native_task_id": task.id,
        "query_tool_id": query_tool_id, "session_id": task.session_id, "query_text": task.query,
        "query_timestamp": datetime.fromtimestamp(task.queried_at.coordinate / 1000, UTC).isoformat(),
        "due_at": datetime.fromtimestamp(task.due_at.coordinate / 1000, UTC).isoformat(),
        "task_status": status, "status": status, "native_state": task.state, "execution_state": execution_state,
        "decision": decision.decision if decision else "", "decision_confidence": decision.confidence if decision else 0,
        "feedback_message_count": len(task.supports), "rollback_status": rollback_status,
        "affected_counts": {"relations": affected.get("association", 0), "stale_paragraphs": 0,
            "episode_sources": 0, "profile_person_ids": 0, "correction_paragraphs": 0,
            "corrected_relations": affected.get("association", 0)}, "native_affected_counts": affected,
        "created_at": task.created_at, "updated_at": task.updated_at, "job_id": task.job_id})
    if detail:
        value.update(json_object({"query_snapshot": {"query": task.query, "hits": [{"hash": ref.id,
            "metadata": {"reference": ref}} for ref in task.references]}, "decision_payload": payload,
            "correction_plan_id": task.correction_plan_id, "rollback_result": task.result if task.result and task.result.action == "rollback" else {},
            "rollback_error": "; ".join(conflict.reason for conflict in task.result.conflicts) if task.result and task.result.action == "rollback" else "",
            "rollback_requested_by": task.rollback_actor, "rollback_reason": task.rollback_reason,
            "rollback_requested_at": task.rollback_recorded_at,
            "rolled_back_at": task.rollback_recorded_at if task.state == "rolled_back" else None,
            "native_task": task}))
    return value
