"""Correction plans preserve model proposals and apply them through domain owners."""

from __future__ import annotations

from typing import TYPE_CHECKING
from uuid import uuid4

from nous_sdk.contracts import (
    AdoptCognition,
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    DomainError,
    EvidenceLink,
    ImportDocument,
    MemoryBody,
    MemoryChange,
    RevisionCause,
)
from nous_sdk.contracts.corrections import (
    ApplyCorrection,
    CorrectionAppliedGroup,
    CorrectionChange,
    CorrectionConflict,
    CorrectionDraft,
    CorrectionModelTarget,
    CorrectionPlan,
    CorrectionPlansPage,
    CorrectionPlansQuery,
    CorrectionResult,
    CorrectionSnapshot,
    PlanCorrection,
    RollbackCorrection,
)
from nous_sdk.contracts.fact_body import FactClaimBody, FactClaimSpec, FactEvidence
from nous_sdk.contracts.facts import FactChange
from nous_sdk.contracts.records import ReadRecords
from nous_sdk.contracts.public import CorrectionInput
from sqlalchemy import JSON, Column, Integer, String, Table, select

from nous.cognition.memory import MemoryService
from nous.identity.access import require_namespace_in
from nous.material.service import observations
from nous.platform.persistence import Authority, ReadView, WriteTx, metadata

from .fact_identity import identity
from .fact_reads import heads_in as fact_heads_in
from .revision_state import RevisionState
from .schema import revisions

if TYPE_CHECKING:
    from nous.platform.models import ModelService

    from .facts import FactService
    from .records import CognitiveRecords
plans = Table(
    "cognitive_corrections",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False, index=True),
    Column("commit_seq", Integer, nullable=False),
    Column("record", JSON, nullable=False),
)


class CorrectionService:
    def __init__(
        self,
        authority: Authority,
        cognition: CognitiveRecords,
        facts: FactService,
        models: ModelService,
        *,
        memories: MemoryService,
    ) -> None:
        self.memories = memories
        self.authority, self.cognition, self.facts, self.models = (
            authority,
            cognition,
            facts,
            models,
        )

    async def correct(self, ctx: CallContext, reference: CognitiveRef, command: CorrectionInput) -> CorrectionResult:
        async with self.authority.read(ctx) as view:
            current = await self.cognition.get_in(view, ctx, reference)
        if isinstance(current.body, MemoryBody):
            from nous_sdk.contracts.corrections import CorrectionEdit
            edits = (CorrectionEdit(reference=reference,body=current.body.model_copy(update={'text':command.content}),reason=command.reason),)
        else:
            edits = ()
        plan = await self.plan(ctx, PlanCorrection(operation_id=command.operation_id, instruction=command.content if not edits else command.reason, targets=(reference,), edits=edits, source=command.source))
        return await self.apply(ctx, ApplyCorrection(operation_id=command.operation_id+':apply',plan_id=plan.id))

    async def _context_in(
        self, view: ReadView | WriteTx, ctx: CallContext, plan: CorrectionPlan
    ) -> CallContext:
        if not ctx.source_groups():
            return ctx
        return (
            await self.cognition.materials.restore_input_in(
                view,
                ctx,
                observation_ids=(plan.source_observation_id,),
            )
        ).context

    async def _stored_in(
        self, view: ReadView | WriteTx, ctx: CallContext, plan_id: str
    ) -> CorrectionPlan:
        await require_namespace_in(view, ctx)
        raw = await view.connection.scalar(
            select(plans.c.record).where(
                plans.c.id == plan_id, plans.c.namespace_id == ctx.namespace_id
            )
        )
        if raw is None:
            raise DomainError("correction_not_found", "Correction plan is unavailable")
        return CorrectionPlan.model_validate(raw)

    async def _require_in(
        self, view: ReadView | WriteTx, ctx: CallContext, plan: CorrectionPlan
    ) -> None:
        if plan.state == "unavailable":
            raise DomainError("source_unavailable", "Correction sources are unavailable")
        from nous.material.service import require_observation_in

        effective = await self._context_in(view, ctx, plan)
        await require_observation_in(view, effective, plan.source_observation_id)
        for target in (*plan.targets, *plan.evidence):
            record = await self.cognition.get_in(view, ctx, target.record.reference)
            if record.purged:
                raise DomainError("source_unavailable", "Correction target was purged")
        for group in plan.applied:
            for snapshot in group.changes:
                record = await self.cognition.get_in(view, effective, snapshot.after.reference)
                if record.purged:
                    raise DomainError("source_unavailable", "Correction result was purged")

    @staticmethod
    def _redacted(plan: CorrectionPlan) -> CorrectionPlan:
        return plan.model_copy(
            update={
                "instruction": "",
                "targets": (),
                "evidence": (),
                "changes": (),
                "explanation": "",
                "questions": (),
                "applied": (),
                "results": (),
                "state": "unavailable",
            }
        )

    async def get(self, ctx: CallContext, plan_id: str) -> CorrectionPlan:
        async with self.authority.read(ctx) as view:
            plan = await self._stored_in(view, ctx, plan_id)
            try:
                await self._require_in(view, ctx, plan)
            except DomainError as error:
                if error.code in {"source_unavailable", "cognition_not_found"}:
                    return self._redacted(plan)
                raise
            return plan

    async def list(self, ctx: CallContext, query: CorrectionPlansQuery) -> CorrectionPlansPage:
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            statement = select(plans.c.record).where(plans.c.namespace_id == ctx.namespace_id)
            if query.cursor is not None:
                statement = statement.where(plans.c.id > query.cursor)
            values = tuple(
                CorrectionPlan.model_validate(raw)
                for raw in (
                    await view.connection.execute(
                        statement.order_by(plans.c.id).limit(query.limit + 1)
                    )
                ).scalars()
            )
            items: list[CorrectionPlan] = []
            for plan in values[: query.limit]:
                try:
                    await self._require_in(view, ctx, plan)
                except DomainError as error:
                    if error.code not in {"source_unavailable", "cognition_not_found"}:
                        raise
                    plan = self._redacted(plan)
                items.append(plan)
            return CorrectionPlansPage(
                items=tuple(items), next_cursor=items[-1].id if len(values) > query.limit else None
            )

    async def _save_in(self, tx: WriteTx, plan: CorrectionPlan) -> CorrectionPlan:
        plan = plan.model_copy(update={"commit_seq": tx.commit_seq})
        await tx.connection.execute(
            plans.update()
            .where(plans.c.id == plan.id)
            .values(commit_seq=tx.commit_seq, record=plan.model_dump(mode="json"))
        )
        return plan

    async def plan(self, ctx: CallContext, command: PlanCorrection) -> CorrectionPlan:
        if not command.instruction.strip() and (not command.edits):
            raise DomainError(
                "correction_intent_required", "Supply a correction intent or explicit edits"
            )
        document = ImportDocument(
            text=command.model_dump_json(),
            filename="correction-intent.json",
            media_type="application/json",
            source_type="administrative_change",
            source=command.source or "correction",
            origin={"caller_id": ctx.caller_id},
        )
        store = self.cognition.materials.content_store
        lease = (
            await store.publish_bytes(document.inline_text().encode(), document.media_type)
            if store
            else None
        )
        try:
            async with self.authority.write(ctx) as tx:
                supports = await self.cognition.read_records_in(tx, ctx, ReadRecords(references=command.supports)) if command.supports else ()
                if command.edits:
                    selected = await self.cognition.read_records_in(tx, ctx, ReadRecords(references=tuple((edit.reference for edit in command.edits))))
                elif command.targets:
                    selected = await self.cognition.read_records_in(tx, ctx, ReadRecords(references=command.targets))
                else:
                    selected = tuple((item.record for item in (await self.cognition.records_in(tx, ctx, command.selection)).items))
                if any((record.body is None or record.deleted for record in selected)):
                    raise DomainError('correction_target_unavailable', 'Restore targets before correcting their content')
                if len({record.reference.id for record in selected}) != len(selected):
                    raise DomainError('correction_selection', 'Select each cognitive identity once')
                admission = await self.cognition.materials.admit_text_in(tx, ctx, document, lease)
                plan = CorrectionPlan(id=str(uuid4()), operation_id=command.operation_id, namespace_id=ctx.namespace_id, created_by=ctx.caller_id, created_at=tx.recorded_at, instruction=command.instruction, targets=tuple((CorrectionModelTarget(key=f'c{index}', record=record) for index, record in enumerate(selected))), evidence=tuple((CorrectionModelTarget(key=f'e{index}', record=record) for index, record in enumerate(supports))), changes=tuple((CorrectionChange(before=record, body=edit.body, reason=edit.reason) for record, edit in zip(selected, command.edits, strict=True))) if command.edits else (), state='ready' if command.edits or not selected else 'generating', source_material_id=admission.material['id'], source_observation_id=admission.event['id'], commit_seq=tx.commit_seq, questions=('Choose the cognitive objects to correct.',) if not selected else ())
                for change in plan.changes:
                    if change.body.kind != change.before.reference.kind:
                        raise DomainError('cognitive_kind_immutable', 'Correction must preserve cognitive kind')
                await tx.connection.execute(plans.insert().values(id=plan.id, namespace_id=plan.namespace_id, commit_seq=tx.commit_seq, record=plan.model_dump(mode='json')))
        finally:
            if store and lease:
                await store.release(lease)
        if plan.state != "generating":
            return plan
        async with self.authority.read(ctx) as view:
            effective = await self._context_in(view, ctx, plan)
        variables: dict[str, object] = {
            "instruction": plan.instruction,
            "targets": [target.model_dump(mode="json") for target in plan.targets],
        }
        if plan.evidence:
            variables["evidence"] = [item.model_dump(mode="json") for item in plan.evidence]
        generated = await self.models.generate(
            effective,
            "correction",
            variables,
            CorrectionDraft,
            source_ids=tuple(
                dict.fromkeys(
                    (
                        plan.source_observation_id,
                        *(
                            root
                            for target in (*plan.targets, *plan.evidence)
                            for root in target.record.source_roots
                        ),
                    )
                )
            ),
        )
        catalog = {target.key: target.record for target in plan.targets}
        if len({item.key for item in generated.output.changes}) != len(generated.output.changes):
            raise DomainError("correction_selection", "A proposal can change each target once")
        changes: list[CorrectionChange] = []
        for suggestion in generated.output.changes:
            before = catalog.get(suggestion.key)
            if before is None:
                raise DomainError(
                    "correction_target_unknown", "Proposal selected an unknown target"
                )
            if suggestion.body.kind != before.reference.kind:
                raise DomainError(
                    "cognitive_kind_immutable", "Correction must preserve cognitive kind"
                )
            changes.append(
                CorrectionChange(before=before, body=suggestion.body, reason=suggestion.reason)
            )
        async with self.authority.write(ctx) as tx:
            current = await self._stored_in(tx, ctx, plan.id)
            await self._require_in(tx, ctx, current)
            if current.state != 'generating':
                return current
            return await self._save_in(tx, current.model_copy(update={'state': 'ready', 'changes': tuple(changes), 'explanation': generated.output.explanation, 'questions': generated.output.questions, 'producer': generated.producer}))

    async def _head_in(
        self, view: ReadView | WriteTx, ctx: CallContext, reference: CognitiveRef
    ) -> CognitiveRecord | None:
        raw = await view.connection.scalar(
            select(revisions.c.record).where(
                revisions.c.namespace_id == ctx.namespace_id,
                revisions.c.id == reference.id,
                revisions.c.head.is_(True),
            )
        )
        return CognitiveRecord.model_validate(raw) if raw is not None else None

    async def _apply_change_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        change: CorrectionChange,
        report: EvidenceLink,
        supports: tuple[CognitiveRecord, ...] = (),
    ) -> CorrectionAppliedGroup:
        before = change.before
        evidence = tuple(
            EvidenceLink(reference=record.reference, role="correction source")
            for record in supports
        )
        if isinstance(change.body, FactClaimBody):
            assert isinstance(before.body, FactClaimBody)
            candidates = await fact_heads_in(tx, ctx, change.body.scope)
            spec = FactClaimSpec.model_validate(
                change.body.model_dump(include=set(FactClaimSpec.model_fields))
            )
            action = (
                "retract"
                if change.body.status == "retracted"
                else "restore"
                if before.body.status == "retracted" and identity(before.body) == identity(spec)
                else "update"
            )
            result, changed, _ = await self.facts.apply_in(
                tx,
                ctx,
                FactChange(
                    action=action,
                    reference=before.reference,
                    reason=change.reason or "Correction",
                    evidence=tuple(FactEvidence(link=link) for link in evidence),
                ),
                spec,
                before,
                candidates,
                (),
                report,
                replace_support=True,
            )
            originals = {record.reference.id: record for record in candidates}
            snapshots = tuple(
                CorrectionSnapshot(before=originals.get(record.reference.id), after=record)
                for record in (result, *changed)
            )
        elif isinstance(change.body, MemoryBody):
            result = await self.memories.change_memory_in(
                tx,
                ctx,
                MemoryChange(
                    reference=before.reference,
                    action="revise",
                    body=change.body,
                    reason=change.reason or "Correction",
                ),
                revision_evidence=(*evidence, report),
            )
            snapshots = (CorrectionSnapshot(before=before, after=result),)
        else:
            result = await self.cognition.adopt_in(
                tx,
                ctx,
                AdoptCognition(
                    id=before.reference.id,
                    expected_revision=before.reference.revision,
                    body=change.body,
                    evidence=(
                        *(
                            link.model_copy(update={"polarity": "boundary"})
                            if link.polarity == "support"
                            else link
                            for link in before.evidence
                        ),
                        *evidence,
                        EvidenceLink(
                            reference=before.reference,
                            role="correction.previous",
                            polarity="boundary",
                        ),
                        report,
                    ),
                    times=before.times,
                ),
                state=RevisionState(
                    lifecycle=before.lifecycle,
                    deleted=before.deleted,
                    cause=RevisionCause(
                        action="correction.apply",
                        actor=ctx.caller_id,
                        reason=change.reason,
                        parent=before.reference,
                    ),
                ),
            )
            snapshots = (CorrectionSnapshot(before=before, after=result),)
        return CorrectionAppliedGroup(target=before.reference, changes=snapshots)

    async def apply(self, ctx: CallContext, command: ApplyCorrection) -> CorrectionResult:
        async with self.authority.write(ctx) as tx:
            plan = await self._stored_in(tx, ctx, command.plan_id)
            await self._require_in(tx, ctx, plan)
            effective = await self._context_in(tx, ctx, plan)
            if plan.state != "ready":
                raise DomainError(
                    "correction_state_conflict", "Correction plan must be ready before adoption"
                )
            conflicts: list[CorrectionConflict] = []
            for change in plan.changes:
                head = await self._head_in(tx, ctx, change.before.reference)
                if (
                    head is None
                    or head.reference != change.before.reference
                    or head.deleted
                    or head.purged
                ):
                    conflicts.append(
                        CorrectionConflict(
                            expected=change.before.reference,
                            current=head.reference if head else None,
                            reason="revision_changed",
                        )
                    )
            if conflicts:
                return CorrectionResult(plan_id=plan.id, action='apply', conflicts=tuple(conflicts), commit_seq=tx.commit_seq)
            report = EvidenceLink(
                observation_id=plan.source_observation_id,
                role="correction.apply",
                polarity="boundary" if plan.evidence else "support",
            )
            supports = tuple(
                [
                    await self.cognition.get_in(tx, effective, item.record.reference)
                    for item in plan.evidence
                ]
            )
            groups: list[CorrectionAppliedGroup] = []
            modified: set[str] = set()
            for change in plan.changes:
                group = await self._apply_change_in(tx, effective, change, report, supports)
                touched = {snapshot.after.reference.id for snapshot in group.changes}
                if touched.intersection(modified):
                    raise DomainError(
                        "correction_target_conflict",
                        "Correction changes overlap through a dependent object",
                    )
                modified.update(touched)
                groups.append(group)
            result = CorrectionResult(
                plan_id=plan.id,
                action="apply",
                records=tuple(snapshot.after for group in groups for snapshot in group.changes),
                commit_seq=tx.commit_seq,
            )
            await self._save_in(
                tx,
                plan.model_copy(
                    update={
                        "state": "applied",
                        "applied": tuple(groups),
                        "results": (*plan.results, result),
                    }
                ),
            )
        return result

    async def rollback(self, ctx: CallContext, command: RollbackCorrection) -> CorrectionResult:
        async with self.authority.write(ctx) as tx:
            plan = await self._stored_in(tx, ctx, command.plan_id)
            await self._require_in(tx, ctx, plan)
            effective = await self._context_in(tx, ctx, plan)
            if plan.state != "applied":
                raise DomainError(
                    "correction_state_conflict", "Only an applied correction can be rolled back"
                )
            restored: list[CognitiveRecord] = []
            conflicts: list[CorrectionConflict] = []
            report = EvidenceLink(
                observation_id=plan.source_observation_id,
                role="correction.rollback",
                polarity="boundary",
            )
            remaining: list[CorrectionAppliedGroup] = []
            for group in plan.applied:
                changed = False
                for snapshot in group.changes:
                    head = await self._head_in(tx, ctx, snapshot.after.reference)
                    if head is None or head.reference != snapshot.after.reference or head.purged:
                        conflicts.append(
                            CorrectionConflict(
                                expected=snapshot.after.reference,
                                current=head.reference if head else None,
                                reason="changed_after_correction",
                            )
                        )
                        changed = True
                if changed:
                    remaining.append(group)
                    continue
                for snapshot in group.changes:
                    current, before = (snapshot.after, snapshot.before)
                    if isinstance(current.body, FactClaimBody):
                        restored.append(
                            await self.facts.restore_correction_in(
                                tx, effective, current, before, report, command.reason
                            )
                        )
                    else:
                        assert before is not None and before.body is not None
                        restored.append(
                            await self.cognition.adopt_in(
                                tx,
                                effective,
                                AdoptCognition(
                                    id=current.reference.id,
                                    expected_revision=current.reference.revision,
                                    body=before.body,
                                    times=before.times,
                                    evidence=(
                                        *before.evidence,
                                        report,
                                        EvidenceLink(
                                            reference=current.reference,
                                            polarity="boundary",
                                            role="correction.rolled_back",
                                        ),
                                    ),
                                ),
                                state=RevisionState(
                                    lifecycle=before.lifecycle,
                                    deleted=before.deleted,
                                    cause=RevisionCause(
                                        action="correction.rollback",
                                        actor=ctx.caller_id,
                                        reason=command.reason,
                                        parent=current.reference,
                                    ),
                                ),
                            )
                        )
            result = CorrectionResult(
                plan_id=plan.id,
                action="rollback",
                records=tuple(restored),
                conflicts=tuple(conflicts),
                commit_seq=tx.commit_seq,
            )
            await self._save_in(
                tx,
                plan.model_copy(
                    update={
                        "state": "applied" if remaining else "rolled_back",
                        "applied": tuple(remaining),
                        "results": (*plan.results, result),
                    }
                ),
            )
        return result

    async def purge_materials_in(self, tx: WriteTx, material_ids: tuple[str, ...]) -> None:
        removed = set(material_ids)
        roots = set(
            (
                await tx.connection.execute(
                    select(observations.c.id).where(observations.c.namespace_id == tx.ctx.namespace_id, observations.c.material_id.in_(material_ids))
                )
            ).scalars()
        )
        for raw in (await tx.connection.execute(select(plans.c.record))).scalars():
            plan = CorrectionPlan.model_validate(raw)
            records = [target.record for target in (*plan.targets, *plan.evidence)]
            records.extend(
                record
                for group in plan.applied
                for snapshot in group.changes
                for record in (snapshot.before, snapshot.after)
                if record
            )
            if plan.source_material_id in removed or any(
                roots.intersection(record.source_roots) for record in records
            ):
                await self._save_in(tx, self._redacted(plan))

    async def purge_references_in(
        self, tx: WriteTx, ctx: CallContext, references: tuple[CognitiveRef, ...]
    ) -> None:
        removed = {reference.id for reference in references}
        for raw in (
            await tx.connection.execute(
                select(plans.c.record).where(plans.c.namespace_id == ctx.namespace_id)
            )
        ).scalars():
            plan = CorrectionPlan.model_validate(raw)
            records = [target.record for target in (*plan.targets, *plan.evidence)]
            records.extend(record for result in plan.results for record in result.records)
            records.extend(
                record
                for group in plan.applied
                for snapshot in group.changes
                for record in (snapshot.before, snapshot.after)
                if record
            )
            if any(record.reference.id in removed for record in records):
                await self._save_in(tx, self._redacted(plan))
