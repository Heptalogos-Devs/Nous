"""Evaluate finite evidence coverage without turning missing observations into success."""

from __future__ import annotations

from typing import TYPE_CHECKING, Literal

from nous_sdk.contracts import (
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    ConcernBody,
    DesireBody,
    DesiredCondition,
    DomainError,
)
from nous_sdk.contracts.common import TimePoint, TimeRange
from nous_sdk.contracts.fact_body import FactClaimBody
from nous_sdk.contracts.motivation import (
    AssessMotivation,
    ConditionAssessment,
    MotivationAssessment,
    MotivationInterpretation,
    MotivationJudgment,
    MotivationSemanticStatus,
)
from pydantic import JsonValue

from nous.platform.errors import NousError

from .fact_identity import normalized
from .lifecycle import selectable

if TYPE_CHECKING:
    from .records import CognitiveRecords
State = Literal["true", "false", "unknown"]


def coverage(
    period: TimeRange, intervals: list[TimeRange]
) -> tuple[tuple[TimeRange, ...], tuple[TimeRange, ...]]:
    if period.start is None or period.end is None:
        return ((), (period,))
    spans: list[tuple[int, int]] = []
    for interval in intervals:
        if interval.domain != period.domain:
            continue
        start = max(period.start, interval.start if interval.start is not None else period.start)
        end = min(period.end, interval.end if interval.end is not None else period.end)
        if start < end:
            spans.append((start, end))
    merged: list[tuple[int, int]] = []
    for start, end in sorted(spans):
        if merged and start <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(merged[-1][1], end))
        else:
            merged.append((start, end))
    gaps: list[TimeRange] = []
    cursor = period.start
    for start, end in merged:
        if cursor < start:
            gaps.append(TimeRange(domain=period.domain, start=cursor, end=start))
        cursor = max(cursor, end)
    if cursor < period.end:
        gaps.append(TimeRange(domain=period.domain, start=cursor, end=period.end))
    return (
        tuple((TimeRange(domain=period.domain, start=start, end=end) for start, end in merged)),
        tuple(gaps),
    )


async def assess_motivation(
    self: CognitiveRecords, ctx: CallContext, command: AssessMotivation | CognitiveRef
) -> MotivationAssessment:
    request = (
        command if isinstance(command, AssessMotivation) else AssessMotivation(reference=command)
    )
    at = request.at or ctx.clock

    def references(condition: DesiredCondition) -> tuple[CognitiveRef, ...]:
        return (
            *condition.evidence,
            *(ref for child in condition.children for ref in references(child)),
        )

    async with self.authority.read(ctx) as view:
        record = await self.get_in(view, ctx, request.reference)
        if not isinstance(record.body, (DesireBody, ConcernBody)):
            raise DomainError("motivation_required", "Assessment requires Desire or Concern")
        body = record.body
        from nous.identity.access import require_namespace_in

        subject = await require_namespace_in(view, ctx, "motivation")
        selected: dict[str, CognitiveRecord] = {}
        for ref in {
            ref.exact(): ref for ref in (*references(body.condition), *request.evidence)
        }.values():
            try:
                evidence = await self.get_in(view, ctx, ref, purpose="current")
            except DomainError as error:
                if error.code == "cognition_inactive":
                    continue
                raise
            if selectable(evidence):
                selected[ref.exact()] = evidence
        root_materials = await self.source_roots_in(view, ctx)

    async def semantic(
        condition: DesiredCondition, evidence: list[CognitiveRecord]
    ) -> tuple[
        dict[str, MotivationJudgment], MotivationSemanticStatus, dict[str, JsonValue] | None
    ]:
        if request.semantic == "off":
            return ({}, "off", None)
        if not evidence:
            return ({}, "not_needed", None)
        if self.models is None:
            if request.semantic == "required":
                raise DomainError(
                    "model_unavailable",
                    "Motivation interpretation requires a model resource",
                    role="motivation",
                )
            return (
                {},
                "unavailable",
                {"code": "model_unavailable", "message": "Motivation model is not configured"},
            )
        catalog = {
            f"e{index}": value
            for index, value in enumerate(sorted(evidence, key=lambda item: item.reference.exact()))
        }
        source_ids = tuple(
            sorted(
                {
                    source
                    for value in (record, *catalog.values())
                    for root in value.source_roots
                    for source in (root, root_materials.get(root, root))
                }
            )
        )
        try:
            generated = await self.models.generate(
                ctx,
                "motivation",
                {
                    "desire": body.description,
                    "predicate": condition.predicate or "",
                    "subject_name": str(subject.display_name),
                    "evidence": [
                        {
                            "key": key,
                            "body": value.body.model_dump(mode="json") if value.body else None,
                            "times": value.times.model_dump(mode="json"),
                        }
                        for key, value in catalog.items()
                    ],
                },
                MotivationInterpretation,
                source_ids=source_ids,
            )
        except (DomainError, NousError) as error:
            if request.semantic == "required":
                raise
            return ({}, "unavailable", {"code": error.code, "message": str(error)})
        returned = generated.output.judgments
        if {item.key for item in returned} != set(catalog) or len(returned) != len(catalog):
            error = DomainError(
                "motivation_output_invalid",
                "Interpretation must judge each supplied evidence key once",
            )
            if request.semantic == "required":
                raise error
            return ({}, "unavailable", {"code": error.code, "message": str(error)})
        judgments: dict[str, MotivationJudgment] = {}
        async with self.authority.read(ctx) as current:
            await self.get_in(current, ctx, request.reference)
            for item in returned:
                reference = catalog[item.key].reference
                try:
                    await self.get_in(current, ctx, reference, purpose="current")
                except DomainError as error:
                    if error.code == "cognition_inactive":
                        continue
                    raise
                judgments[reference.exact()] = MotivationJudgment(
                    reference=reference,
                    state=item.state,
                    confidence=item.confidence,
                    reason=item.reason,
                    producer=generated.producer,
                )
        return (judgments, "ready", None)

    def applicable(evidence: CognitiveRecord, period: TimeRange) -> bool:
        validity = evidence.times.validity
        if validity is not None:
            return (
                validity.domain == period.domain
                and (validity.end is None or period.start is None or validity.end > period.start)
                and (validity.start is None or period.end is None or validity.start < period.end)
            )
        instant = evidence.times.event or evidence.times.observation
        return instant is not None and period.contains(instant)

    async def evaluate(condition: DesiredCondition) -> ConditionAssessment:
        period = (
            request.period
            or condition.hold
            or TimeRange(domain=at.domain, start=at.coordinate, end=at.coordinate + 1)
        )
        if period.domain != at.domain:
            return ConditionAssessment(
                predicate=condition.predicate,
                operator=condition.operator,
                mode=condition.mode,
                state="unknown",
                satisfaction="unknown",
                gaps=(period,),
                reason="Evidence time domain has no applicable mapping",
            )
        if condition.operator != "predicate":
            children = tuple([await evaluate(child) for child in condition.children])
            satisfactions = [child.satisfaction for child in children]
            if condition.operator == "all":
                state: State = (
                    "false"
                    if "false" in satisfactions
                    else "unknown"
                    if "unknown" in satisfactions
                    else "true"
                )
            elif condition.operator == "any":
                state = (
                    "true"
                    if "true" in satisfactions
                    else "unknown"
                    if "unknown" in satisfactions
                    else "false"
                )
            else:
                state = (
                    "false"
                    if satisfactions[0] == "true"
                    else "true"
                    if satisfactions[0] == "false"
                    else "unknown"
                )
            statuses = {child.semantic_status for child in children}
            status: MotivationSemanticStatus = (
                "unavailable"
                if "unavailable" in statuses
                else "ready"
                if "ready" in statuses
                else "not_needed"
                if "not_needed" in statuses
                else "off"
            )
            return ConditionAssessment(
                operator=condition.operator,
                mode=condition.mode,
                state=state,
                satisfaction=state,
                children=children,
                semantic_status=status,
                evidence=tuple(
                    {ref.exact(): ref for child in children for ref in child.evidence}.values()
                ),
            )
        values: dict[str, MotivationJudgment] = {}
        pending: list[CognitiveRecord] = []
        candidates = [
            selected[ref.exact()]
            for ref in {
                ref.exact(): ref for ref in (*condition.evidence, *request.evidence)
            }.values()
            if ref.exact() in selected and applicable(selected[ref.exact()], period)
        ]
        for evidence in candidates:
            claim = evidence.body
            if isinstance(claim, FactClaimBody) and claim.status != "active":
                continue
            text = (
                normalized(claim.value_text)
                if isinstance(claim, FactClaimBody)
                and normalized(claim.fact_key) == normalized(condition.predicate or "")
                else ""
            )
            if text in {"true", "false"}:
                value: State = "true" if text == "true" else "false"
                if isinstance(claim, FactClaimBody) and claim.polarity == "negative":
                    value = "false" if value == "true" else "true"
                values[evidence.reference.exact()] = MotivationJudgment(
                    reference=evidence.reference,
                    state=value,
                    confidence=claim.confidence
                    if isinstance(claim, FactClaimBody) and claim.confidence is not None
                    else 1,
                    reason="Current boolean fact and explicit applicability",
                )
            else:
                pending.append(evidence)
        interpreted, status, semantic_error = await semantic(condition, pending)
        values.update(interpreted)
        events: list[tuple[int, State, CognitiveRef]] = []
        intervals: list[TimeRange] = []
        positive: list[TimeRange] = []
        negative: list[TimeRange] = []
        used: list[CognitiveRef] = []
        for evidence in candidates:
            judgment = values.get(evidence.reference.exact())
            if judgment is None or judgment.state == "unknown":
                continue
            value = judgment.state
            used.append(evidence.reference)
            validity = evidence.times.validity
            if validity is not None:
                intervals.append(validity)
                (positive if value == "true" else negative).append(validity)
                start = validity.start if validity.start is not None else period.start
                if start is not None:
                    start = max(start, period.start) if period.start is not None else start
                    events.append((start, value, evidence.reference))
            else:
                instant = evidence.times.event or evidence.times.observation
                assert instant is not None
                events.append((instant.coordinate, value, evidence.reference))
                span = TimeRange(
                    domain=instant.domain, start=instant.coordinate, end=instant.coordinate + 1
                )
                intervals.append(span)
                (positive if value == "true" else negative).append(span)
        covered, gaps = coverage(period, intervals)
        has_true, has_false = (bool(positive), bool(negative))
        transition = None
        if condition.mode == "achieve":
            state = "true" if has_true else "false" if not gaps and has_false else "unknown"
        elif condition.mode == "maintain":
            state = "false" if has_false else "true" if not gaps and has_true else "unknown"
        elif condition.mode == "avoid":
            state = "false" if has_true else "true" if not gaps and has_false else "unknown"
        else:
            changes = [
                (before, after)
                for before in events
                for after in events
                if before[1] == "true"
                and after[1] == "false"
                and (before[0] < after[0])
                and (
                    not any(
                        span.contains(TimePoint(domain=period.domain, coordinate=after[0]))
                        for span in positive
                    )
                )
            ]
            if changes:
                before, after = min(changes, key=lambda item: (item[1][0], -item[0][0]))
                boundary = max(
                    (
                        span.end
                        for span in positive
                        if span.end is not None
                        and span.start is not None
                        and (span.start <= before[0] < span.end <= after[0])
                    ),
                    default=before[0] + 1,
                )
                transition = TimeRange(domain=period.domain, start=boundary, end=after[0] + 1)
            state = (
                "true"
                if changes
                else "false"
                if not gaps and has_true and (not has_false)
                else "unknown"
            )
        reason = (
            "Observed evidence supports the condition result"
            if state != "unknown"
            else "Observed evidence has temporal gaps"
            if used
            else "No applicable evidence in the requested interval"
        )
        return ConditionAssessment(
            predicate=condition.predicate,
            operator="predicate",
            mode=condition.mode,
            state=state,
            satisfaction=state,
            evidence=tuple(used),
            coverage=covered,
            gaps=gaps,
            reason=reason,
            judgments=tuple(values.values()),
            semantic_status=status,
            semantic_error=semantic_error,
            transition_window=transition,
        )

    result = await evaluate(body.condition)

    def judged_references(assessment: ConditionAssessment) -> tuple[CognitiveRef, ...]:
        return (
            *assessment.evidence,
            *(judgment.reference for judgment in assessment.judgments),
            *(reference for child in assessment.children for reference in judged_references(child)),
        )

    async with self.authority.read(ctx) as current:
        await self.get_in(current, ctx, request.reference)
        for reference in dict.fromkeys(judged_references(result)):
            await self.get_in(current, ctx, reference, purpose="current")
    return MotivationAssessment(**result.model_dump(), reference=request.reference, evaluated_at=at)
