"""Explicitly saved assessments preserve exact conditions and evidence, with live source grants."""

from __future__ import annotations

from typing import TYPE_CHECKING
from uuid import uuid4

import sqlalchemy as sa
from nous_sdk.contracts import (
    CallContext,
    CognitiveRef,
    ConcernBody,
    DesireBody,
    DesiredCondition,
    DomainError,
)
from nous_sdk.contracts.motivation import (
    AssessMotivation,
    ConditionAssessment,
    MotivationAssessment,
    MotivationAssessmentsPage,
    MotivationAssessmentsQuery,
    SavedMotivationAssessment,
)

from nous.identity.access import require_namespace_in
from nous.platform.persistence import ReadView, WriteTx, metadata

from .motivation import assess_motivation as evaluate_motivation

if TYPE_CHECKING:
    from .records import CognitiveRecords
assessments = sa.Table(
    "cognitive_motivation_assessments",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("namespace_id", sa.String, nullable=False, index=True),
    sa.Column("record", sa.JSON, nullable=False),
    sa.Column("commit_seq", sa.Integer, nullable=False),
)
sa.Index(
    "ix_motivation_assessment_history",
    assessments.c.namespace_id,
    assessments.c.commit_seq,
    assessments.c.id,
)


def condition_references(condition: DesiredCondition) -> tuple[CognitiveRef, ...]:
    return (
        *condition.evidence,
        *(ref for child in condition.children for ref in condition_references(child)),
    )


def judged_references(value: ConditionAssessment) -> tuple[CognitiveRef, ...]:
    return (
        *value.evidence,
        *(judgment.reference for judgment in value.judgments),
        *(ref for child in value.children for ref in judged_references(child)),
    )


async def authorize_in(
    self, view: ReadView | WriteTx, ctx: CallContext, saved: SavedMotivationAssessment
) -> None:
    await require_namespace_in(view, ctx, "motivation")
    for ref in saved.references:
        await self.get_in(view, ctx, ref, purpose="exact")


class MotivationService:
    def __init__(self, records: CognitiveRecords):
        self.records = records

    async def motivation_assessment(
        self, ctx: CallContext, assessment_id: str
    ) -> SavedMotivationAssessment:
        async with self.records.authority.read(ctx) as view:
            raw = await view.connection.scalar(
                sa.select(assessments.c.record).where(
                    assessments.c.id == assessment_id,
                    assessments.c.namespace_id == ctx.namespace_id,
                )
            )
            if raw is None:
                raise DomainError(
                    "motivation_assessment_not_found",
                    "Saved assessment is unavailable in this Subject",
                )
            saved = SavedMotivationAssessment.model_validate(raw)
            await authorize_in(self.records, view, ctx, saved)
            return saved

    async def latest_motivation_assessment_in(
        self, view: ReadView | WriteTx, ctx: CallContext, reference: CognitiveRef
    ) -> SavedMotivationAssessment | None:
        raw = await view.connection.scalar(
            sa.select(assessments.c.record)
            .where(
                assessments.c.namespace_id == ctx.namespace_id,
                assessments.c.record["request"]["reference"]["id"].as_string() == reference.id,
                assessments.c.record["request"]["reference"]["kind"].as_string() == reference.kind,
                assessments.c.record["request"]["reference"]["revision"].as_integer()
                == reference.revision,
            )
            .order_by(assessments.c.commit_seq.desc(), assessments.c.id)
            .limit(1)
        )
        if raw is None:
            return None
        saved = SavedMotivationAssessment.model_validate(raw)
        try:
            await authorize_in(self.records, view, ctx, saved)
        except DomainError as error:
            if error.code in {
                "source_forbidden",
                "source_unavailable",
                "cognition_not_found",
                "cognition_inactive",
            }:
                return None
            raise
        return saved

    async def assess_motivation(
        self, ctx: CallContext, command: AssessMotivation | CognitiveRef
    ) -> MotivationAssessment:
        request = (
            command
            if isinstance(command, AssessMotivation)
            else AssessMotivation(reference=command)
        )
        if not request.save:
            return await evaluate_motivation(self.records, ctx, request)
        receipt = await self.records.authority.receipt(
            ctx,
            request.operation_id,
            operation_kind="motivation.assess",
            scope=ctx.namespace_id,
            payload=request,
        )
        if receipt is not None:
            result = MotivationAssessment.model_validate(receipt.result)
            assert result.assessment_id is not None
            return (await self.motivation_assessment(ctx, result.assessment_id)).assessment
        result = await evaluate_motivation(self.records, ctx, request)
        async with self.records.authority.write(
            ctx,
            request.operation_id,
            operation_kind="motivation.assess",
            scope=ctx.namespace_id,
            payload=request,
        ) as tx:
            if tx.replayed:
                result = MotivationAssessment.model_validate(tx.replay_result)
                assert result.assessment_id is not None
                raw = await tx.connection.scalar(
                    sa.select(assessments.c.record).where(assessments.c.id == result.assessment_id)
                )
                saved = SavedMotivationAssessment.model_validate(raw)
                await authorize_in(self.records, tx, ctx, saved)
                return result
            await require_namespace_in(tx, ctx, "motivation")
            target = await self.records.get_in(tx, ctx, request.reference)
            if not isinstance(target.body, (DesireBody, ConcernBody)):
                raise DomainError("motivation_required", "Assessment requires Desire or Concern")
            for reference in dict.fromkeys(judged_references(result)):
                await self.records.get_in(tx, ctx, reference, purpose="current")
            refs = tuple(
                dict.fromkeys(
                    (
                        request.reference,
                        *condition_references(target.body.condition),
                        *request.evidence,
                        *judged_references(result),
                    )
                )
            )
            roots: set[str] = set()
            for ref in refs:
                evidence = await self.records.get_in(tx, ctx, ref, purpose="exact")
                roots.update(evidence.source_roots)
            material_roots = await self.records.source_roots_in(tx, ctx, purpose="management")
            source_ids = tuple(sorted(roots | {material_roots.get(root, root) for root in roots}))
            identity = str(uuid4())
            result = result.model_copy(
                update={
                    "assessment_id": identity,
                    "saved_at": tx.recorded_at,
                    "commit_seq": tx.commit_seq,
                }
            )
            saved = SavedMotivationAssessment(
                id=identity,
                namespace_id=ctx.namespace_id,
                caller_id=ctx.caller_id,
                recorded_at=tx.recorded_at,
                commit_seq=tx.commit_seq,
                request=request,
                condition=target.body.condition,
                description=target.body.description,
                assessment=result,
                references=refs,
                source_ids=source_ids,
            )
            await tx.connection.execute(
                assessments.insert().values(
                    id=identity,
                    namespace_id=ctx.namespace_id,
                    record=saved.model_dump(mode="json"),
                    commit_seq=tx.commit_seq,
                )
            )
            return tx.result(result)

    async def motivation_assessments(
        self, ctx: CallContext, query: MotivationAssessmentsQuery
    ) -> MotivationAssessmentsPage:
        statement = sa.select(assessments.c.record).where(
            assessments.c.namespace_id == ctx.namespace_id
        )
        if query.reference is not None:
            statement = statement.where(
                assessments.c.record["request"]["reference"]["id"].as_string()
                == query.reference.id,
                assessments.c.record["request"]["reference"]["kind"].as_string()
                == query.reference.kind,
                assessments.c.record["request"]["reference"]["revision"].as_integer()
                == query.reference.revision,
            )
        if query.cursor is not None:
            sequence, separator, identity = query.cursor.partition(":")
            if not separator or not sequence.isdecimal() or (not identity):
                raise DomainError("motivation_cursor_invalid", "Assessment cursor is invalid")
            statement = statement.where(
                sa.or_(
                    assessments.c.commit_seq < int(sequence),
                    sa.and_(assessments.c.commit_seq == int(sequence), assessments.c.id > identity),
                )
            )
        async with self.records.authority.read(ctx) as view:
            await require_namespace_in(view, ctx, "motivation")
            rows = list(
                (
                    await view.connection.execute(
                        statement.order_by(assessments.c.commit_seq.desc(), assessments.c.id).limit(
                            query.limit + 1
                        )
                    )
                ).scalars()
            )
            values: list[SavedMotivationAssessment] = []
            for raw in rows[: query.limit]:
                saved = SavedMotivationAssessment.model_validate(raw)
                try:
                    await authorize_in(self.records, view, ctx, saved)
                except DomainError as error:
                    if error.code not in {
                        "source_forbidden",
                        "source_unavailable",
                        "cognition_not_found",
                        "cognition_inactive",
                    }:
                        raise
                else:
                    values.append(saved)
        last = (
            SavedMotivationAssessment.model_validate(rows[query.limit - 1])
            if len(rows) > query.limit
            else None
        )
        return MotivationAssessmentsPage(
            items=tuple(values),
            next_cursor=f"{last.commit_seq}:{last.id}" if last is not None else None,
        )

    async def purge_motivation_assessments_in(
        self,
        tx: WriteTx,
        *,
        material_ids: tuple[str, ...] = (),
        references: tuple[CognitiveRef, ...] = (),
    ) -> None:
        ids = {ref.id for ref in references}
        for raw in (
            await tx.connection.execute(
                sa.select(assessments.c.record).where(
                    assessments.c.namespace_id == tx.ctx.namespace_id
                )
            )
        ).scalars():
            saved = SavedMotivationAssessment.model_validate(raw)
            if not ids.intersection(ref.id for ref in saved.references) and (
                not set(material_ids).intersection(saved.source_ids)
            ):
                continue
            await tx.connection.execute(assessments.delete().where(assessments.c.id == saved.id))
            await self.records.authority.redact_receipts_in(
                tx, lambda _value: True, commit_sequences=(saved.commit_seq,)
            )
