"""Context admission owned behavior."""

from __future__ import annotations

from hashlib import sha256
from typing import TYPE_CHECKING
from uuid import uuid4

from nous_wave.contracts import (
    CallContext,
    DomainError,
)
from nous_wave.contracts.context import (
    CognitiveTarget,
    ContextAdmission,
    ContextAdmissionCommand,
    ContextPreparation,
    ContextProjection,
    ContextReport,
    ContextReportResult,
)
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select, update

from .models import contexts, feedback, preparations, reports, sessions, tracks

if TYPE_CHECKING:
    from .managed import ManagedContexts


async def admit_managed(
    self: ManagedContexts, ctx: CallContext, command: ContextAdmissionCommand
) -> ContextAdmission:
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="context.admit_managed",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            replay = ContextAdmission.model_validate(tx.replay_result)
            await self.require_targets_in(tx, ctx, replay.sources)
            return replay
        value = await self.read_preparation_in(tx, ctx, command.preparation_id)
        preparation = ContextPreparation.model_validate(value["preparation"])
        if preparation.state != "prepared":
            raise DomainError("context_not_prepared", "Only a pending preparation can be admitted")
        projection = ContextProjection.model_validate(value["full_projection"])
        expected = tuple(segment.key for segment in projection.segments)
        if command.applied_segment_ids != expected:
            raise DomainError(
                "context_input_mismatch",
                "Applied segment identities do not match the prepared complete input",
            )
        targets = tuple(target for segment in projection.segments for target in segment.sources)
        # The current caller scope and the accepted preparation scope must both hold.
        await self.require_targets_in(tx, ctx, targets)
        accepted_scope = TypeAdapter(tuple[str, ...] | None).validate_python(value["source_scope"])
        accepted_constraints = TypeAdapter(tuple[tuple[str, ...], ...]).validate_python(
            value.get("source_constraints", ())
        )
        await self.require_targets_in(
            tx,
            ctx.model_copy(
                update={
                    "allowed_source_ids": accepted_scope,
                    "source_constraints": accepted_constraints,
                }
            ),
            targets,
        )
        prior = (
            await tx.connection.execute(
                select(tracks.c.record).where(tracks.c.id == projection.cursor.track_id)
            )
        ).scalar_one_or_none()
        if preparation.response.kind == "append":
            current = ContextProjection.model_validate(prior) if prior is not None else None
            if current is None or current.cursor != preparation.request.known_cursor:
                raise DomainError(
                    "context_cursor_conflict",
                    "The applied context cursor no longer matches its admitted track",
                )
        result = ContextAdmission(
            id=str(uuid4()),
            projection_id=projection.id,
            namespace_id=ctx.namespace_id,
            subject_id=ctx.require_subject(),
            session_id=projection.session_id,
            consumer_id=projection.consumer_id,
            instance_id=projection.instance_id,
            invocation_id=command.invocation_id,
            rendered_digest=command.rendered_digest,
            history_generation=command.history_generation,
            segments=expected,
            sources=targets,
            measurement=command.measurement,
            recorded_at=tx.recorded_at,
            commit_seq=tx.commit_seq,
        )
        references = [
            target.reference.model_dump(mode="json")
            for target in targets
            if isinstance(target, CognitiveTarget)
        ]
        record = {
            "id": result.id,
            "kind": "managed",
            "executor": projection.consumer_id,
            "purpose": projection.usage.purpose,
            "references": references,
            "token_upper_bound": command.measurement.input_tokens,
            "commit_seq": tx.commit_seq,
            "managed_admission": result.model_dump(mode="json"),
            "caller_id": ctx.caller_id,
        }
        await tx.connection.execute(
            contexts.insert().values(
                id=result.id,
                namespace_id=ctx.namespace_id,
                subject_id=ctx.subject_id,
                record=record,
            )
        )
        if prior is None:
            await tx.connection.execute(
                tracks.insert().values(
                    id=projection.cursor.track_id,
                    namespace_id=ctx.namespace_id,
                    subject_id=ctx.subject_id,
                    record=projection.model_dump(mode="json"),
                )
            )
        else:
            await tx.connection.execute(
                update(tracks)
                .where(tracks.c.id == projection.cursor.track_id)
                .values(record=projection.model_dump(mode="json"))
            )
        value["preparation"] = TypeAdapter(JsonValue).validate_json(
            preparation.model_copy(update={"state": "admitted"}).model_dump_json()
        )
        await tx.connection.execute(
            update(preparations).where(preparations.c.id == preparation.id).values(record=value)
        )
        from .lifecycle import session_key

        current_session = await self.session_in(tx, ctx, projection.session_id)
        revised_session = current_session.model_copy(
            update={
                "history_generation": command.history_generation,
                "history_invalidated": False,
                "revision": current_session.revision + 1,
                "commit_seq": tx.commit_seq,
            }
        )
        await tx.connection.execute(
            update(sessions)
            .where(sessions.c.id == session_key(ctx, projection.session_id))
            .values(record=revised_session.model_dump(mode="json"))
        )
        for target in targets:
            event = {
                "id": str(uuid4()),
                "context_id": result.id,
                "target": target.model_dump(mode="json"),
                "kind": "model_contact",
                "invocation_id": result.invocation_id,
                "commit_seq": tx.commit_seq,
            }
            await tx.connection.execute(
                feedback.insert().values(id=event["id"], context_id=result.id, record=event)
            )
        return tx.result(result)


async def report_managed(
    self: ManagedContexts, ctx: CallContext, command: ContextReport
) -> ContextReportResult:
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="context.report",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            return ContextReportResult.model_validate(tx.replay_result)
        value = (
            await tx.connection.execute(
                select(contexts.c.record).where(
                    contexts.c.id == command.admission_id,
                    contexts.c.namespace_id == ctx.namespace_id,
                    contexts.c.subject_id == ctx.subject_id,
                )
            )
        ).scalar_one_or_none()
        if (
            value is None
            or "managed_admission" not in value
            or value.get("caller_id") != ctx.caller_id
        ):
            raise DomainError(
                "context_admission_not_found", "Managed context admission is unavailable"
            )
        admission = ContextAdmission.model_validate(value["managed_admission"])
        if admission.invocation_id != command.invocation_id:
            raise DomainError(
                "context_invocation_mismatch",
                "Outcome belongs to a different actual invocation",
            )
        for use in command.uses:
            if use.target not in admission.sources:
                raise DomainError(
                    "context_use_not_admitted",
                    "Reported use was not present in the admitted model input",
                )
            await self.require_targets_in(tx, ctx, (use.target,))
            event_key = sha256(
                f"{ctx.caller_id}\0{admission.id}\0{use.event_id}".encode()
            ).hexdigest()
            event = {
                "id": event_key,
                "event_id": use.event_id,
                "context_id": admission.id,
                "target": use.target.model_dump(mode="json"),
                "kind": "actual_use",
                "use_kind": use.kind,
                "invocation_id": command.invocation_id,
                "occurred_at": command.occurred_at.isoformat(),
                "commit_seq": tx.commit_seq,
            }
            existing_event = await tx.connection.scalar(
                select(feedback.c.record).where(feedback.c.id == event_key)
            )
            if existing_event is None:
                await tx.connection.execute(
                    feedback.insert().values(id=event_key, context_id=admission.id, record=event)
                )
            elif (
                existing_event.get("target") != event["target"]
                or existing_event.get("use_kind") != use.kind
            ):
                raise DomainError(
                    "context_use_conflict",
                    "Use event identity already names a different observation",
                )
        if command.uses:
            from .lifecycle import session_key

            current_session = await self.session_in(tx, ctx, admission.session_id)
            updated_residents = tuple(
                item.model_copy(
                    update={
                        "last_meaningful_use_at": max(
                            command.occurred_at, item.last_meaningful_use_at or item.entered_at
                        )
                    }
                )
                if any(use.target == item.target for use in command.uses)
                else item
                for item in current_session.residents
            )
            revised_session = current_session.model_copy(
                update={
                    "residents": updated_residents,
                    "revision": current_session.revision + 1,
                    "commit_seq": tx.commit_seq,
                }
            )
            await tx.connection.execute(
                update(sessions)
                .where(sessions.c.id == session_key(ctx, admission.session_id))
                .values(record=revised_session.model_dump(mode="json"))
            )
        result = ContextReportResult(
            id=str(uuid4()),
            admission_id=admission.id,
            invocation_id=command.invocation_id,
            outcome=command.outcome,
            occurred_at=command.occurred_at,
            uses=command.uses,
            commit_seq=tx.commit_seq,
        )
        await tx.connection.execute(
            reports.insert().values(
                id=result.id,
                namespace_id=ctx.namespace_id,
                subject_id=ctx.subject_id,
                admission_id=admission.id,
                record=result.model_dump(mode="json"),
            )
        )
        return tx.result(result)


async def read_admission(
    self: ManagedContexts, ctx: CallContext, identity: str
) -> ContextAdmission:
    async with self.authority.read(ctx) as view:
        value = await view.connection.scalar(
            select(contexts.c.record).where(
                contexts.c.id == identity,
                contexts.c.namespace_id == ctx.namespace_id,
                contexts.c.subject_id == ctx.subject_id,
            )
        )
        if (
            value is None
            or value.get("caller_id") != ctx.caller_id
            or "managed_admission" not in value
        ):
            raise DomainError(
                "context_admission_not_found", "Managed context admission is unavailable"
            )
        admission = ContextAdmission.model_validate(value["managed_admission"])
        await self.require_targets_in(view, ctx, admission.sources)
        return admission
