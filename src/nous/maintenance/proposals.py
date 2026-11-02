"""Retained model documents and explicit human edits compile into independently reviewable groups."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.growth import GrowthDraft, GrowthEntry, GrowthSnapshot
from nous_sdk.contracts.maintenance import (
    EditMaintenanceProposal,
    MaintenanceEdit,
    MaintenanceGroup,
    MaintenanceProposal,
    MaintenanceRun,
)
from sqlalchemy import update

from nous.cognition.lifecycle import selectable
from nous.platform.persistence import OperationConflict, ReadView, WriteTx

from .groups import compile_proposal
from .schema import runs

if TYPE_CHECKING:
    from .service import MaintenanceService


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


async def edit_proposal(
    self: MaintenanceService, ctx: CallContext, run_id: str, command: EditMaintenanceProposal
) -> MaintenanceRun:
    async with self.authority.write(ctx) as tx:
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
                "producer": record.proposal.producer if record.proposal is not None else None,
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
            producer=document.producer,
        )
        updated = record.model_copy(
            update={
                "proposal": document,
                "status": "proposed",
                "waiting_reason": None,
                "draft": compiled_draft(document, groups),
                "groups": groups,
                "edits": (*record.edits, edit),
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
    return await self.read(ctx, run_id)
