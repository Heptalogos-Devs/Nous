"""Retained model documents and explicit human edits compile into independently reviewable groups."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.growth import GrowthDraft, GrowthEntry, GrowthSnapshot
from nous_sdk.contracts.maintenance import (
    CaptureMaintenanceOutput,
    EditMaintenanceProposal,
    MaintenanceEdit,
    MaintenanceGroup,
    MaintenanceProposal,
    MaintenanceRun,
)
from pydantic import JsonValue, TypeAdapter
from pydantic_core import from_json
from sqlalchemy import update

from nous.cognition.lifecycle import selectable
from nous.platform.persistence import OperationConflict, ReadView, WriteTx

from .groups import compile_proposal
from .schema import runs

if TYPE_CHECKING:
    from .service import MaintenanceService


def document_from_output(
    value: JsonValue,
    *,
    raw_text: str | None,
    call_id: str,
    complete: bool,
    errors: tuple[dict[str, JsonValue], ...],
) -> MaintenanceProposal:
    diagnostics = list(errors)
    if not isinstance(value, dict) and raw_text is not None:
        try:
            value = TypeAdapter(JsonValue).validate_python(from_json(raw_text, allow_partial=True))
        except ValueError as error:
            diagnostics.append({"type": "proposal_document_unreadable", "message": str(error)})
    data = value if isinstance(value, dict) else {}
    raw_actions = data.get("actions", [])
    if not isinstance(raw_actions, list):
        diagnostics.append(
            {"type": "proposal_actions_invalid", "message": "Proposal actions must be an array"}
        )
        raw_actions = []
    questions = data.get("questions", [])
    summary, rationale = data.get("summary"), data.get("rationale")
    return MaintenanceProposal(
        summary=summary if isinstance(summary, str) else None,
        rationale=rationale if isinstance(rationale, str) else None,
        actions=tuple(raw_actions),
        questions=tuple(item for item in questions if isinstance(item, str))
        if isinstance(questions, list)
        else (),
        source_call_id=call_id,
        complete=complete,
        validation_errors=tuple(diagnostics),
    )


def compiled_draft(
    document: MaintenanceProposal, groups: tuple[MaintenanceGroup, ...]
) -> GrowthDraft | None:
    actions = tuple(
        action for group in groups if group.draft is not None for action in group.draft.actions
    )
    if len(actions) > 100:
        return None
    return GrowthDraft(
        summary=document.summary or "Proposal groups",
        rationale=document.rationale or "Retained proposal input",
        actions=actions,
    )


async def working_snapshot_in(
    self: MaintenanceService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    snapshot: GrowthSnapshot,
    groups: tuple[MaintenanceGroup, ...],
) -> GrowthSnapshot:
    catalog = {entry.key: entry.record for entry in snapshot.entries}
    watermark = snapshot.watermark
    for group in groups:
        if group.status != "adopted":
            continue
        for binding in group.bindings:
            actual = await self.cognition.get_in(view, ctx, binding.reference)
            for key, previous in tuple(catalog.items()):
                if previous.reference.id == actual.reference.id:
                    catalog.pop(key)
                    if selectable(actual):
                        catalog[key] = actual
            if selectable(actual):
                catalog[binding.key] = actual
            watermark = max(watermark, actual.commit_seq)
    return snapshot.model_copy(
        update={
            "entries": tuple(
                GrowthEntry(key=key, record=record) for key, record in catalog.items()
            ),
            "watermark": watermark,
        }
    )


async def capture_retained(
    self: MaintenanceService, ctx: CallContext, run_id: str, command: CaptureMaintenanceOutput
) -> MaintenanceRun:
    current = await self.read(ctx, run_id)
    if current.catalog is None or current.purged_at is not None:
        raise DomainError(
            "maintenance_proposal_unavailable",
            "The proposal requires its retained authorized source catalog",
        )
    retained = await self.models.retained_output(ctx, command.call_id)
    expected_role = "reflect" if current.command.kind == "growth" else current.command.kind
    if retained.role != expected_role or not await self.models.ledger.owns_invocation_call(
        ctx, expected_role, "maintenance:" + current.id, command.call_id
    ):
        raise DomainError(
            "maintenance_call_mismatch",
            "The retained call does not belong to this proposal invocation",
        )
    if current.command.kind in {"growth", "journal", "reflect", "dream"}:
        from pydantic import ValidationError

        from .task_proposals import document as task_document

        async with self.authority.read(ctx) as view:
            snapshot = await self.snapshot_in(view, ctx, current.catalog)
        try:
            document = task_document(
                current,
                snapshot,
                retained.raw_json if retained.raw_json is not None else retained.saved_output,
                command.call_id,
            )
        except (ValidationError, DomainError) as error:
            document = MaintenanceProposal(
                complete=False,
                source_call_id=command.call_id,
                validation_errors=({"code": "task_output_invalid", "message": str(error)},),
            )
    else:
        document = document_from_output(
            retained.raw_json if retained.raw_json is not None else retained.saved_output,
            raw_text=retained.raw_text,
            call_id=command.call_id,
            complete=retained.complete,
            errors=retained.validation_errors,
        )
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="maintenance.capture_retained",
        scope=ctx.namespace_id,
        payload={"run_id": run_id, "command": command},
    ) as tx:
        if not tx.replayed:
            record = await self.run_in(tx, ctx, run_id)
            if record.revision != command.expected_revision:
                raise OperationConflict("Maintenance proposal revision changed")
            if record.catalog is None or record.purged_at is not None:
                raise DomainError(
                    "maintenance_proposal_unavailable", "The source catalog is no longer retained"
                )
            snapshot = await self.snapshot_in(tx, ctx, record.catalog)
            groups = await compile_proposal(
                self, tx, ctx, document, snapshot, role=record.command.kind, previous=record.groups
            )
            updated = record.model_copy(
                update={
                    "proposal": document,
                    "status": "proposed",
                    "waiting_reason": None,
                    "draft": compiled_draft(document, groups),
                    "groups": groups,
                    "producer_call_id": command.call_id,
                    "generation_call_ids": tuple(
                        dict.fromkeys((*record.generation_call_ids, command.call_id))
                    ),
                    "questions": document.questions,
                    "review": None,
                    "review_history": (
                        *record.review_history,
                        *((record.review,) if record.review else ()),
                    ),
                    "revision": record.revision + 1,
                }
            )
            await tx.connection.execute(
                update(runs)
                .where(runs.c.id == run_id)
                .values(record=updated.model_dump(mode="json"))
            )
            tx.result({"run_id": run_id})
    return await self.read(ctx, run_id)


async def edit_proposal(
    self: MaintenanceService, ctx: CallContext, run_id: str, command: EditMaintenanceProposal
) -> MaintenanceRun:
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="maintenance.edit",
        scope=ctx.namespace_id,
        payload={"run_id": run_id, "command": command},
    ) as tx:
        if not tx.replayed:
            record = await self.run_in(tx, ctx, run_id)
            if record.revision != command.expected_revision:
                raise OperationConflict("Maintenance proposal revision changed")
            if (
                record.catalog is None
                or record.purged_at is not None
                or record.review is not None
                and record.review.decision == "reject"
            ):
                raise DomainError(
                    "maintenance_proposal_unavailable",
                    "The retained proposal is finalized or its sources are unavailable",
                )
            snapshot = await self.snapshot_in(tx, ctx, record.catalog)
            document = command.proposal.model_copy(
                update={
                    "origin": "human",
                    "complete": True,
                    "source_call_id": record.proposal.source_call_id
                    if record.proposal is not None
                    else record.producer_call_id,
                    "validation_errors": (),
                }
            )
            groups = await compile_proposal(
                self, tx, ctx, document, snapshot, role=record.command.kind, previous=record.groups
            )
            edit = MaintenanceEdit(
                caller_id=ctx.caller_id,
                recorded_at=tx.recorded_at,
                commit_seq=tx.commit_seq,
                reason=command.reason,
                source_call_id=document.source_call_id,
            )
            updated = record.model_copy(
                update={
                    "proposal": document,
                    "status": "proposed",
                    "waiting_reason": None,
                    "draft": compiled_draft(document, groups),
                    "groups": groups,
                    "edits": (*record.edits, edit),
                    "producer_call_id": None,
                    "questions": document.questions,
                    "review": None,
                    "review_history": (
                        *record.review_history,
                        *((record.review,) if record.review else ()),
                    ),
                    "revision": record.revision + 1,
                }
            )
            await tx.connection.execute(
                update(runs)
                .where(runs.c.id == run_id)
                .values(record=updated.model_dump(mode="json"))
            )
            tx.result({"run_id": run_id})
    return await self.read(ctx, run_id)
