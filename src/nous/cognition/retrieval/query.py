"""Bind a user's query, deliver exact evidence and optionally arrange it."""

from __future__ import annotations

from contextlib import AsyncExitStack
from typing import TYPE_CHECKING, cast

from pydantic import JsonValue, TypeAdapter

from nous.application.contracts import CallContext, EventDeliveryError, Notice, Progress
from nous.cognition.contracts import NamespaceRef
from nous.cognition.material_contracts import MaterialSelection
from nous.cognition.steward.contracts import ArrangeReading
from nous.common.errors import NousError
from nous.common.operations import OperationIssue, report_outcome
from nous.content.contracts import ResultRef
from nous.wave.contracts import (
    SearchCoverage,
    WaveQuery,
    WaveSearchResult,
)

from .adoption import read_saved_evidence
from .binding import QueryBinder, merge_controls
from .context import ContextPreparer
from .contracts import (
    CognitiveGroupContext,
    ContinuationLink,
    DeliveryUsage,
    EvidenceDelivery,
    EvidenceMaterial,
    EvidencePassage,
    QueryAtom,
    QueryControls,
    QueryCoverage,
    QueryIntent,
    QueryRequest,
    RetrievalOutcome,
    SaveEvidence,
    UsedContext,
)
from .nousql import parse_nousql

if TYPE_CHECKING:
    from nous.cognition.concepts import ConceptOperations
    from nous.cognition.materials import Materials
    from nous.cognition.scope import NamespaceOperations
    from nous.cognition.steward.operations import StewardOperations
    from nous.cognition.work.operations import WorkOperations
    from nous.content.artifacts import ArtifactStore
    from nous.content.results import ResultStore
    from nous.storage.database import Database
    from nous.temporal.domains import TimeService
    from nous.wave.operations import WaveOperations

    from .adoption import EvidenceAdoption
    from .operations import CognitiveRetrieval


def _coverage(value: SearchCoverage) -> QueryCoverage:
    summary: QueryCoverage = {
        "truncated": value.truncated,
        "recall_exhausted": value.recall_exhausted,
    }
    if value.ranked_candidates is not None:
        summary["matched"] = value.ranked_candidates > 0
    elif value.returned_groups:
        summary["matched"] = True
    if value.reason is not None:
        summary["reason"] = value.reason
    return summary


class QueryExecutor:
    """Own the complete query and evidence-adoption operations."""

    def __init__(
        self,
        database: Database,
        scope: NamespaceOperations,
        artifacts: ArtifactStore,
        results: ResultStore,
        wave: WaveOperations,
        concepts: ConceptOperations,
        materials: Materials,
        work: WorkOperations,
        steward: StewardOperations,
        time: TimeService,
        views: CognitiveRetrieval,
        adoption: EvidenceAdoption,
    ) -> None:
        self.database, self.scope = database, scope
        self.results, self.wave = results, wave
        self.concepts, self.materials, self.steward = concepts, materials, steward
        self.time, self.views, self.adoption = time, views, adoption
        self.context = ContextPreparer(work, scope, materials, results, artifacts)

    async def save_evidence(self, request: SaveEvidence, call: CallContext) -> ResultRef:
        evidence = request.evidence.model_copy(deep=True)
        evidence = await self.adoption.validate(evidence)
        sources = tuple(source for item in evidence.materials for source in item.sources())
        return await self.adoption.save(
            evidence, call, sources=sources, retain_snapshot=request.retain_snapshot
        )

    async def _direct(
        self,
        request: QueryRequest,
        controls: QueryControls,
        namespace: NamespaceRef,
    ) -> EvidenceDelivery:
        if (
            controls.times
            or controls.time_clauses
            or controls.filters
            or controls.prefer
            or controls.avoid
            or controls.sort
        ):
            raise NousError(
                "input.unsupported_query_control",
                "Direct material selection does not accept these controls",
            )
        budget = request.delivery.budget
        batch = await self.materials.read_budgeted(
            tuple(selector.value for selector in controls.selectors),
            budget,
            namespace=namespace,
            return_types=controls.return_types,
            limit=controls.limit,
        )
        materials: list[EvidenceMaterial] = []
        continuation = [
            ContinuationLink(selection=item, relation="read_remaining") for item in batch.remaining
        ]
        passage_number = 0
        for reading in batch.readings:
            view = reading.material
            passages: tuple[EvidencePassage, ...] = ()
            if reading.selections or reading.shared:
                passage_number += 1
                passages = (
                    EvidencePassage(
                        key=f"E{passage_number}", views=reading.selections, shared=reading.shared
                    ),
                )
            materials.append(
                EvidenceMaterial(material=view.ref, title=view.title, passages=passages)
            )
            if controls.explore:
                continuation.extend(
                    ContinuationLink(
                        selection=MaterialSelection(material=item.target), relation=item.kind
                    )
                    for item in await self.materials.follow_view(view)
                )
        intent = QueryIntent(
            original_text=request.text,
            aspects=request.aspects,
            namespace=namespace,
            controls=request.controls if request.controls != QueryControls() else None,
            expression=request.expression,
        )
        return EvidenceDelivery(
            intent=intent,
            formed_at=self.time.clock.utc_now(),
            materials=tuple(materials),
            continuation=tuple(continuation),
            budget=budget,
            delivery_usage=DeliveryUsage(unit=batch.unit, amount=batch.used),
        )

    async def _thinking_links(self, result: WaveSearchResult) -> tuple[ContinuationLink, ...]:
        if not result.thinking or not result.thinking.get("stages"):
            return ()
        rows = [
            (stage["name"], stage.get("branch"), entry)
            for stage in result.thinking.get("stages", ())
            for entry in stage["results"]
        ]
        keys = tuple(
            dict.fromkeys(
                key
                for _, _, entry in rows
                for key in (
                    entry["candidate"],
                    *entry["parents"],
                )
            )
        )
        descriptors = await self.wave.describe_candidates(result.snapshot, keys)
        groups = await self.wave.describe_groups(
            result.snapshot, tuple(dict.fromkeys(item.group_key for item in descriptors))
        )
        references = {
            item.ref.key: CognitiveGroupContext.model_validate(
                groups[item.group_key].context
            ).material
            for item in descriptors
        }
        links: list[ContinuationLink] = []
        for stage, branch, entry in rows:
            links.append(
                ContinuationLink(
                    selection=MaterialSelection(material=references[entry["candidate"]]),
                    relation=entry["relation"],
                    via=tuple(dict.fromkeys(references[key] for key in entry["parents"])),
                    stage=stage,
                    branch=branch,
                    score=entry["score"],
                    route=entry["route"],
                )
            )
        return tuple(links)

    async def run(self, request: QueryRequest, call: CallContext) -> RetrievalOutcome:
        binder = QueryBinder(
            self.materials, self.concepts, self.wave, self.time, self.time.clock.utc_now()
        )
        expression = request.expression or parse_nousql(request.text)
        root_controls = merge_controls(request.controls, expression.controls)
        if isinstance(expression, QueryAtom) and expression.selectors:
            root_controls = root_controls.model_copy(
                update={
                    "selectors": tuple(
                        dict.fromkeys((*root_controls.selectors, *expression.selectors))
                    )
                }
            )
        steward_policy = request.delivery.steward or call.settings.values.cognition.search.steward
        await call.events.emit(Progress(stage="retrieval", completed=0, total=1))
        target = None
        anchor = request.anchor
        direct = (
            isinstance(expression, QueryAtom)
            and not expression.text.strip()
            and not (request.expression is not None and request.text.strip())
            and bool(root_controls.selectors)
            and all(item.kind == "ref" for item in root_controls.selectors)
        )
        if direct and anchor and anchor.view == "anchored":
            raise NousError(
                "input.unsupported_query_control",
                "An explicit material reference cannot use an anchored search view",
            )
        prior_ref = anchor.prior_delivery if anchor and not direct else None
        if isinstance(prior_ref, str):
            prior_ref = (await self.results.refs((prior_ref,)))[0]
        if anchor and anchor.view == "anchored":
            snapshot_ref = anchor.snapshot
            if snapshot_ref is None and prior_ref is not None:
                snapshot_ref = (await read_saved_evidence(self.results, prior_ref)).snapshot
            if snapshot_ref is None:
                raise NousError(
                    "input.snapshot_required",
                    "An anchored query requires a snapshot or prior delivery",
                )
            snapshot = await self.wave.describe(snapshot_ref)
            from nous.wave.contracts import CognitionOwner

            if not isinstance(snapshot.owner, CognitionOwner):
                raise NousError("cognition.namespace_mismatch", "Snapshot is not a cognitive view")
            target = NamespaceRef(
                instance_id=snapshot.ref.instance_id, id=snapshot.owner.namespace_id
            )
        else:
            snapshot = None
        snapshot_issue: OperationIssue | None = None
        selected = None
        if direct:
            namespace = await self.scope.resolve_for(request.namespace)
            async with self.database.read() as tx:
                ns = await self.scope.get_in(tx, namespace.id)
        else:
            selected = await self.views.resolve_options(
                request.namespace,
                None,
                root_controls.include_review,
                root_controls.include_exploration,
                target=target,
                explore=root_controls.explore,
            )
            if snapshot is None:
                ns, snapshot, current_generation = await self.views.current_view(selected)
                if snapshot.captured_generation != current_generation:
                    snapshot_issue = OperationIssue(
                        stage="retrieval",
                        code="cognition.snapshot_changed",
                        message="Material changed; results reflect an earlier view. Search again for the latest material.",
                        details={
                            "captured_generation": snapshot.captured_generation,
                            "observed_generation": current_generation,
                        },
                        retryable=True,
                    )
            else:
                async with self.database.read() as tx:
                    ns = await self.scope.get_in(tx, selected.namespace)
                self.views.check_owner(snapshot, ns)
        async with AsyncExitStack() as stack:
            if snapshot is not None:
                await stack.enter_async_context(self.wave.snapshots.hold_reference(snapshot.ref))
            retrieval_issues: tuple[OperationIssue, ...] = ()
            steward_context: tuple[dict[str, str], ...] = ()
            if direct:
                evidence = await self._direct(request, root_controls, ns.ref)
            else:
                assert snapshot is not None and selected is not None
                context_selection = request.context
                if prior_ref is not None:
                    context_selection = context_selection.model_copy(
                        update={
                            "prior_deliveries": (
                                *context_selection.prior_deliveries,
                                prior_ref,
                            )
                        }
                    )
                fragments, sources_by_key = await self.context.prepare(
                    context_selection, ns.ref, anchor.selections if anchor else (), call
                )
                bound = await binder.bind_expression(
                    expression, request.controls, ns.ref, snapshot.ref.identifier, call, root=True
                )
                result = await self.wave.query(
                    WaveQuery(
                        snapshot=snapshot.ref.identifier,
                        text=request.text
                        if request.expression is not None and request.text.strip()
                        else binder.expression_text(expression),
                        expression=bound,
                        aspects=request.aspects,
                        grouping="document",
                        top_k=root_controls.limit,
                        passages_per_group=call.settings.values.cognition.search.passages_per_record,
                        include_trace=root_controls.diagnostics,
                        context=fragments,
                        prefer=root_controls.prefer,
                        avoid=root_controls.avoid,
                        sort=root_controls.sort,
                        explore=root_controls.explore,
                        delivery_budget=request.delivery.budget,
                    ),
                    call=call,
                    selection=selected.selection,
                )
                selected_context = result.selected_context
                steward_context = tuple(
                    {"role": item.role, "text": item.text} for item in selected_context if item.text
                )
                used: list[UsedContext] = []
                for item in selected_context:
                    source = sources_by_key.get(item.key)
                    if item.content is not None:
                        source = MaterialSelection(
                            material=source.material
                            if source is not None
                            else item.content.artifact,
                            selections=(item.content,),
                        )
                    used.append(
                        UsedContext(
                            key=item.key,
                            source=source,
                            text=None if item.content is not None else item.text,
                            role=item.role,
                            stage=item.stage,
                            parent_key=item.parent_key or item.key,
                            contribution=item.contribution,
                        )
                    )
                intent = QueryIntent(
                    original_text=request.text,
                    aspects=request.aspects,
                    namespace=ns.ref,
                    controls=request.controls if request.controls != QueryControls() else None,
                    context=tuple(used),
                    expression=request.expression,
                )
                materials: list[EvidenceMaterial] = []
                group_diagnostics: list[JsonValue] = []
                passage_diagnostics: dict[str, JsonValue] = {}
                passage_number = 0
                continuation: list[ContinuationLink] = []
                contexts = tuple(
                    CognitiveGroupContext.model_validate(hit.group.context) for hit in result.groups
                )
                views = await self.materials.resolve_many(
                    tuple(context.material for context in contexts), namespace=ns.ref
                )
                for hit, context, view in zip(result.groups, contexts, views, strict=True):
                    reading_parts: list[EvidencePassage] = []
                    for passage in hit.passages:
                        actual_views = (*passage.fragments, *passage.support_context)
                        if (
                            any(
                                content.artifact != view.body.artifact
                                for content in (
                                    *(fragment.source for fragment in actual_views),
                                    *passage.scored_ranges,
                                    *passage.shared_context,
                                )
                            )
                            if view.body
                            else True
                        ):
                            raise NousError(
                                "cognition.selection_mismatch",
                                "Delivered content does not belong to its material",
                            )
                        passage_number += 1
                        key = f"E{passage_number}"
                        reading_parts.append(
                            EvidencePassage(
                                key=key,
                                views=actual_views,
                                shared=passage.shared_context,
                                assessment=passage.assessment,
                            )
                        )
                        if root_controls.diagnostics:
                            passage_diagnostics[key] = passage.model_dump(
                                mode="json",
                                exclude={
                                    "fragments",
                                    "support_context",
                                    "shared_context",
                                    "assessment",
                                },
                            )
                    materials.append(
                        EvidenceMaterial(
                            material=view.ref,
                            title=view.title,
                            passages=tuple(reading_parts),
                            rank=hit.rank,
                            score=hit.basis.ranking_score,
                            score_kind=hit.basis.score_kind,
                            delivery_status=hit.delivery_status,
                            needs_review=context.needs_review,
                        )
                    )
                    if root_controls.diagnostics:
                        group_diagnostics.append(
                            {"rank": hit.rank, "basis": hit.basis.model_dump(mode="json")}
                        )
                    if not reading_parts:
                        continuation.append(
                            ContinuationLink(
                                selection=MaterialSelection(material=view.ref), relation="read"
                            )
                        )
                    if root_controls.explore:
                        continuation.extend(
                            ContinuationLink(
                                selection=MaterialSelection(material=item.target),
                                relation=item.kind,
                            )
                            for item in await self.materials.follow_view(view)
                        )
                if root_controls.explore:
                    continuation.extend(await self._thinking_links(result))
                retrieval_issues = tuple(
                    TypeAdapter(OperationIssue).validate_python(issue) for issue in result.issues
                )
                if snapshot_issue is not None:
                    retrieval_issues = (*retrieval_issues, snapshot_issue)
                evaluation = result.context_summary.get("evaluation")
                effective_budget = result.budget
                usage_unit = effective_budget.unit if effective_budget else "characters"
                usage_amount = result.context_summary.get(f"delivered_{usage_unit}")
                branches = {item.name: _coverage(item.coverage) for item in result.branch_coverage}
                if len(branches) < 2 or all(
                    value == next(iter(branches.values())) for value in branches.values()
                ):
                    branches = {}
                evidence = EvidenceDelivery(
                    intent=intent,
                    formed_at=self.time.clock.utc_now(),
                    snapshot=snapshot.ref,
                    captured_generation=snapshot.captured_generation,
                    materials=tuple(materials),
                    coverage=_coverage(result.coverage),
                    branch_coverage=branches,
                    continuation=tuple(continuation),
                    program=result.program,
                    unavailable_capabilities=result.unavailable_capabilities
                    if result.fallback_from
                    else (),
                    fallback_from=result.fallback_from,
                    budget=effective_budget,
                    delivery_usage=DeliveryUsage(unit=usage_unit, amount=usage_amount)
                    if isinstance(usage_amount, int) and not isinstance(usage_amount, bool)
                    else None,
                    evaluation=cast(dict[str, JsonValue], evaluation)
                    if isinstance(evaluation, dict)
                    else None,
                    issues=retrieval_issues,
                    diagnostics={
                        **(result.diagnostics or {}),
                        "groups": group_diagnostics,
                        "passages": passage_diagnostics,
                        "coverage": result.coverage.model_dump(mode="json"),
                        "branch_coverage": [
                            item.model_dump(mode="json") for item in result.branch_coverage
                        ],
                    }
                    if root_controls.diagnostics
                    else None,
                )
            await call.events.emit(Progress(stage="retrieval", completed=1, total=1))
            evidence_ref = None
            sources = ()
            if request.save or steward_policy in {"optional", "required"}:
                sources = tuple(
                    source for material in evidence.materials for source in material.sources()
                )
                evidence_ref = await self.adoption.save(
                    evidence,
                    call,
                    sources=sources,
                    retain_snapshot=bool(anchor and anchor.view == "anchored"),
                )
            reading = reading_ref = None
            issues: list[OperationIssue] = []
            if retrieval_issues:
                report_outcome(
                    "partial",
                    issues=retrieval_issues,
                    completed_refs=(evidence_ref,) if evidence_ref else (),
                )
                for issue in retrieval_issues:
                    await call.events.emit(Notice(issue=issue))
            if steward_policy in {"optional", "required"}:
                assert evidence_ref is not None
                try:
                    arranged = await self.steward.arrange_sources(
                        ArrangeReading(
                            evidence=evidence_ref,
                            namespace=ns.ref,
                            question=evidence.intent.original_text,
                            aspects=evidence.intent.aspects,
                            context=steward_context,
                            purpose=request.delivery.purpose,
                            detail=request.delivery.detail,
                            output_language=request.delivery.output_language,
                        ),
                        sources,
                        call,
                    )
                    reading, reading_ref = arranged.brief, arranged.reading_ref
                except EventDeliveryError:
                    raise
                except Exception as error:
                    if steward_policy == "required":
                        raise
                    issue = OperationIssue(
                        stage="steward",
                        code=error.code if isinstance(error, NousError) else "steward.failed",
                        message=str(error),
                        details=(
                            {
                                **error.details,
                                **(
                                    {"retry_after": error.retry_after}
                                    if error.retry_after is not None
                                    else {}
                                ),
                            }
                            if isinstance(error, NousError)
                            else {}
                        ),
                        retryable=error.retryable if isinstance(error, NousError) else False,
                        completed_refs=(evidence_ref.model_dump(mode="json"),),
                    )
                    issues.append(issue)
                    report_outcome(
                        "partial",
                        issues=(*retrieval_issues, *issues),
                        completed_refs=(evidence_ref,),
                    )
                    await call.events.emit(Notice(issue=issue))
            return RetrievalOutcome(
                evidence=evidence,
                evidence_ref=evidence_ref,
                reading=reading,
                reading_ref=reading_ref,
                issues=tuple(issues),
            )
