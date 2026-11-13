"""Bind a user's query, deliver exact evidence and optionally arrange it."""

from __future__ import annotations

from typing import TYPE_CHECKING

from pydantic import JsonValue, TypeAdapter

from nous.application.contracts import CallContext, EventDeliveryError, Notice, Progress
from nous.cognition.concept_contracts import ConceptRef
from nous.cognition.contracts import NamespaceRef
from nous.cognition.material_contracts import (
    MaterialRef,
    MaterialSelection,
)
from nous.cognition.steward.contracts import ArrangeReading
from nous.common.errors import NousError
from nous.common.operations import OperationIssue, report_outcome
from nous.content.contracts import (
    ContentSelection,
    ResultOutputRef,
    SourceSelection,
    is_text_media_type,
)
from nous.wave.contracts import (
    ContextFragment,
    DeliveredText,
    DeliveryStatus,
    SearchCoverage,
    SelectedContext,
    SnapshotDescriptor,
    WaveQuery,
    WaveSearchResult,
)

from .binding import QueryBinder, merge_controls
from .context import ContextPreparer
from .contracts import (
    BoundQueryBranch,
    BoundQueryControls,
    CognitiveGroupContext,
    ContinuationLink,
    DeliveryUsage,
    EvidenceDelivery,
    PassageEvidence,
    QueryAtom,
    QueryControls,
    QueryExpression,
    QueryIntent,
    QueryRequest,
    RetrievalOutcome,
    RetrievedMaterial,
    UsedContext,
)
from .nousql import parse_nousql

if TYPE_CHECKING:
    from .operations import CognitiveRetrieval


class QueryExecutor:
    """Turn one query into an evidence delivery, preserving its completed outputs."""

    def __init__(self, owner: CognitiveRetrieval):
        self.owner = owner
        self.binder = QueryBinder(
            owner.materials,
            owner.annotations.concepts,
            owner.wave,
            owner.time,
            owner.artifacts.clock.utc_now(),
        )
        self.context = ContextPreparer(
            owner.work, owner.scope, owner.materials, owner.results, owner.artifacts
        )

    async def _direct(
        self,
        request: QueryRequest,
        controls: QueryControls,
        namespace: NamespaceRef,
        snapshot: SnapshotDescriptor,
        call: CallContext,
    ) -> tuple[EvidenceDelivery, list[PassageEvidence]]:
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
        refs = tuple(
            dict.fromkeys(
                [
                    await self.owner.materials.resolve_identifier(
                        selector.value, namespace=namespace
                    )
                    for selector in controls.selectors
                ]
            )
        )
        views = list(await self.owner.materials.resolve_many(refs, namespace=namespace))
        if controls.return_types:
            views = [
                view
                for view in views
                if view.kind in controls.return_types
                or (
                    "cognition" in controls.return_types
                    and view.kind in self.owner.materials.searchable_kinds()
                )
            ]
        limited = views[: controls.limit] if controls.limit else views
        budget = request.delivery.budget
        if budget and budget.unit == "tokens":
            raise NousError(
                "wave.capability_unavailable", "A tokenizer is required for a token delivery budget"
            )
        remaining = budget.amount if budget else None
        materials: list[RetrievedMaterial] = []
        passages: list[PassageEvidence] = []
        continuation: list[ContinuationLink] = []
        body_truncated = False
        for index, view in enumerate(limited, 1):
            keys: tuple[str, ...] = ()
            status: DeliveryStatus = "no_candidates"
            if view.body is not None and remaining == 0:
                body_truncated = True
                status = "budget_exhausted"
            if view.body is not None and (remaining is None or remaining > 0):
                artifact = await self.owner.artifacts.get(view.body.artifact)
                if is_text_media_type(artifact.media_type):
                    part = (await self.owner.artifacts.read_selections((view.body,)))[0]
                    if remaining is None:
                        text = part.text
                    elif budget is not None and budget.unit == "bytes":
                        text = part.text.encode("utf-8")[:remaining].decode(
                            "utf-8", errors="ignore"
                        )
                    else:
                        text = part.text[:remaining]
                    if len(text) < len(part.text):
                        body_truncated = True
                        if text:
                            continuation.append(
                                ContinuationLink(target=view.ref, relation="read_remaining")
                            )
                        else:
                            status = "budget_exhausted"
                    if text:
                        key = f"E{len(passages) + 1}"
                        content = ContentSelection(
                            artifact=part.content.artifact,
                            selection=SourceSelection(
                                start_byte=part.start_byte,
                                end_byte=part.start_byte + len(text.encode("utf-8")),
                            ),
                        )
                        passages.append(
                            PassageEvidence(
                                key=key,
                                material=view.ref,
                                fragments=(DeliveredText(content=content, text=text),),
                                scored_ranges=(content,),
                                selection_reason="explicit_reference",
                            )
                        )
                        keys = (key,)
                        status = "selected"
                        if remaining is not None:
                            remaining -= (
                                len(text.encode("utf-8"))
                                if budget is not None and budget.unit == "bytes"
                                else len(text)
                            )
            materials.append(
                RetrievedMaterial(
                    material=view, rank=index, passage_keys=keys, delivery_status=status
                )
            )
            if not keys:
                continuation.append(ContinuationLink(target=view.ref, relation="read"))
            if controls.explore:
                continuation.extend(
                    ContinuationLink(target=item.target, relation=item.kind)
                    for item in view.relations
                )
        count = len(materials)
        coverage = SearchCoverage(
            requested_groups=controls.limit,
            returned_groups=count,
            eligible_groups_known=len(views),
            allowed_groups=len(views),
            recall_exhausted=True,
            truncated=count < len(views) or body_truncated,
            reason="delivery_budget" if body_truncated else "explicit_reference",
            lanes={},
            lane_limit_unit="groups",
            group_scan_limit=len(views),
            passages_per_group=1,
            candidates=0,
            total_candidates=0,
            max_union_candidates=0,
            top_k=controls.limit or max(count, 1),
            offered_candidates=0,
            ranked_candidates=0,
            unique_candidates=0,
            candidate_limit_reached=False,
            top_k_truncated=count < len(views),
        )
        intent = QueryIntent(
            original_text=request.text,
            aspects=request.aspects,
            constraints=BoundQueryControls(controls=controls, namespace=namespace),
            bound_materials=refs,
        )
        return EvidenceDelivery(
            intent=intent,
            formed_at=self.owner.artifacts.clock.utc_now(),
            snapshot=snapshot.ref,
            captured_generation=snapshot.captured_generation,
            materials=tuple(materials),
            passages=tuple(passages),
            coverage=coverage,
            continuation=tuple(continuation),
            program="direct",
            budget=budget,
            delivery_usage=DeliveryUsage(
                unit="bytes" if budget and budget.unit == "bytes" else "characters",
                amount=sum(
                    len(fragment.text.encode("utf-8"))
                    if budget and budget.unit == "bytes"
                    else len(fragment.text)
                    for part in passages
                    for fragment in part.fragments
                ),
            ),
        ), passages

    async def _run_branch(
        self,
        expression: QueryExpression,
        inherited: QueryControls,
        namespace: NamespaceRef,
        snapshot: str,
        context: tuple[ContextFragment, ...],
        request: QueryRequest,
        call: CallContext,
    ) -> tuple[
        WaveSearchResult,
        tuple[MaterialRef, ...],
        tuple[ConceptRef, ...],
        tuple[BoundQueryBranch, ...],
    ]:
        controls = merge_controls(inherited, expression.controls)
        bound, materials, concepts, branches = await self.binder.bind_expression(
            expression, inherited, namespace, snapshot, call, root=True
        )
        budget = request.delivery.budget
        result = await self.owner.wave.query(
            WaveQuery(
                snapshot=snapshot,
                text=request.text
                if request.expression is not None and request.text.strip()
                else self.binder.expression_text(expression),
                expression=bound,
                program=None,
                grouping="document",
                top_k=controls.limit,
                passages_per_group=call.settings.values.cognition.search.passages_per_record,
                include_trace=controls.diagnostics,
                context=context,
                prefer=controls.prefer,
                avoid=controls.avoid,
                sort=controls.sort,
                explore=controls.explore,
                delivery_budget=budget,
            ),
            call=call,
        )
        return result, materials, concepts, branches

    async def _thinking_links(self, result: WaveSearchResult) -> tuple[ContinuationLink, ...]:
        if not result.thinking or not result.thinking.get("stages"):
            return ()
        stages = TypeAdapter(list[dict[str, object]]).validate_python(result.thinking["stages"])
        rows: list[tuple[str, dict[str, object]]] = []
        for stage in stages:
            for entry in TypeAdapter(list[dict[str, object]]).validate_python(
                stage.get("results", [])
            ):
                rows.append((str(stage.get("name", "")), entry))
        keys = tuple(
            dict.fromkeys(
                str(key)
                for _, entry in rows
                for key in (
                    entry.get("candidate"),
                    *TypeAdapter(list[str]).validate_python(entry.get("parents", [])),
                )
                if key is not None
            )
        )
        descriptors = await self.owner.wave.describe_candidates(result.snapshot, keys)
        groups = await self.owner.wave.describe_groups(
            result.snapshot, tuple(dict.fromkeys(item.group_key for item in descriptors))
        )
        references = {
            item.ref.key: CognitiveGroupContext.model_validate(
                groups[item.group_key].context
            ).material
            for item in descriptors
        }
        links: list[ContinuationLink] = []
        for stage, entry in rows:
            candidate = str(entry.get("candidate"))
            parents = TypeAdapter(list[str]).validate_python(entry.get("parents", []))
            route = TypeAdapter(dict[str, str | None]).validate_python(entry.get("route") or {})
            score = entry.get("score")
            links.append(
                ContinuationLink(
                    target=references[candidate],
                    relation=str(entry.get("relation") or "thinking"),
                    via=tuple(dict.fromkeys(references[key] for key in parents)),
                    stage=stage,
                    score=float(score) if isinstance(score, int | float) else None,
                    route=route,
                )
            )
        return tuple(links)

    async def run(self, request: QueryRequest, call: CallContext) -> RetrievalOutcome:
        owner = self.owner
        expression = request.expression or parse_nousql(request.text)
        root_controls = merge_controls(request.controls, expression.controls)
        steward_policy = request.delivery.steward or call.settings.values.cognition.search.steward
        await call.events.emit(Progress(stage="retrieval", completed=0, total=1))
        target = None
        anchor = request.anchor
        if anchor and anchor.view == "anchored":
            snapshot_ref = anchor.snapshot
            if snapshot_ref is None and anchor.prior_delivery is not None:
                artifact = await owner.results.resolve_output(
                    ResultOutputRef(result=anchor.prior_delivery, output="evidence")
                )
                prior = EvidenceDelivery.model_validate_json(
                    await owner.artifacts.read_text(artifact)
                )
                snapshot_ref = prior.snapshot
            if snapshot_ref is None:
                raise NousError(
                    "input.snapshot_required",
                    "An anchored query requires a snapshot or prior delivery",
                )
            snapshot = await owner.wave.describe(snapshot_ref)
            from nous.wave.contracts import CognitionOwner

            if not isinstance(snapshot.owner, CognitionOwner):
                raise NousError("cognition.namespace_mismatch", "Snapshot is not a cognitive view")
            target = NamespaceRef(
                instance_id=snapshot.ref.instance_id, id=snapshot.owner.namespace_id
            )
        else:
            snapshot = None
        selected = await owner.resolve_options(
            request.namespace,
            None,
            root_controls.include_review,
            root_controls.include_exploration,
            target=target,
        )
        if snapshot is None:
            ns, snapshot, _ = await owner.current_view(selected)
        else:
            async with owner.database.read() as tx:
                ns = await owner.scope.get_in(tx, selected.namespace)
            owner.check_owner(snapshot, ns)
        async with owner.wave.snapshots.hold_reference(snapshot.ref):
            retrieval_issues: tuple[OperationIssue, ...] = ()
            context_selection = request.context
            if anchor and anchor.prior_delivery is not None:
                context_selection = context_selection.model_copy(
                    update={
                        "prior_deliveries": (
                            *context_selection.prior_deliveries,
                            anchor.prior_delivery,
                        )
                    }
                )
            fragments, used_by_key = await self.context.prepare(
                context_selection, request.text, ns.ref, anchor.selections if anchor else (), call
            )
            direct = (
                isinstance(expression, QueryAtom)
                and not expression.text.strip()
                and bool(root_controls.selectors)
                and all(item.kind == "ref" for item in root_controls.selectors)
            )
            if direct:
                evidence, passages = await self._direct(
                    request, root_controls, ns.ref, snapshot, call
                )
            else:
                result, bound_materials, bound_concepts, bound_branches = await self._run_branch(
                    expression,
                    request.controls,
                    ns.ref,
                    snapshot.ref.identifier,
                    fragments,
                    request,
                    call,
                )
                selected_context = TypeAdapter(tuple[SelectedContext, ...]).validate_python(
                    result.context_summary.get("selected_context", [])
                )
                used: list[UsedContext] = []
                for item in selected_context:
                    value = used_by_key.get(item.key)
                    if value is None:
                        value = UsedContext(
                            key=item.key,
                            source=MaterialSelection(
                                material=item.content.artifact, selections=(item.content,)
                            )
                            if item.content
                            else None,
                            text=item.text,
                            role=item.role,
                            stage=item.stage,
                            parent_key=item.parent_key or item.key,
                        )
                    used.append(value.model_copy(update={"contribution": item.contribution}))
                intent = QueryIntent(
                    original_text=request.text,
                    aspects=request.aspects,
                    constraints=BoundQueryControls(
                        controls=root_controls.model_copy(
                            update={
                                "times": await self.binder.time_conditions(root_controls),
                                "time_clauses": (),
                            }
                        ),
                        namespace=ns.ref,
                        branches=bound_branches,
                    ),
                    context=tuple(used),
                    bound_materials=bound_materials,
                    bound_concepts=bound_concepts,
                    expression=expression,
                )
                materials: list[RetrievedMaterial] = []
                passages: list[PassageEvidence] = []
                continuation: list[ContinuationLink] = []
                contexts = tuple(
                    CognitiveGroupContext.model_validate(hit.group.context) for hit in result.groups
                )
                views = await owner.materials.resolve_many(
                    tuple(context.material for context in contexts), namespace=ns.ref
                )
                for hit, context, view in zip(result.groups, contexts, views, strict=True):
                    view = view.model_copy(update={"attributes": context.attributes})
                    keys: list[str] = []
                    for passage in hit.passages:
                        if (
                            any(
                                content.artifact != view.body.artifact
                                for content in (
                                    *(fragment.content for fragment in passage.fragments),
                                    *(fragment.content for fragment in passage.support_context),
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
                        key = f"E{len(passages) + 1}"
                        keys.append(key)
                        passages.append(
                            PassageEvidence(
                                key=key,
                                material=view.ref,
                                candidate=passage.candidate,
                                fragments=passage.fragments,
                                support_context=passage.support_context,
                                scored_ranges=passage.scored_ranges,
                                shared_context=passage.shared_context,
                                scores=passage.scores,
                                assessment=passage.assessment,
                                selection_reason=passage.reason,
                            )
                        )
                    materials.append(
                        RetrievedMaterial(
                            material=view,
                            rank=hit.rank,
                            basis=hit.basis,
                            passage_keys=tuple(keys),
                            delivery_status=hit.delivery_status,
                        )
                    )
                    if not keys:
                        continuation.append(ContinuationLink(target=view.ref, relation="read"))
                    if root_controls.explore:
                        continuation.extend(
                            ContinuationLink(target=item.target, relation=item.kind)
                            for item in view.relations
                        )
                if root_controls.explore:
                    continuation.extend(await self._thinking_links(result))
                retrieval_issues = tuple(
                    TypeAdapter(OperationIssue).validate_python(issue) for issue in result.issues
                )
                evaluation = result.context_summary.get("evaluation")
                effective_budget = (
                    result.program_run.budget if result.program_run else request.delivery.budget
                )
                usage_unit = (
                    "bytes"
                    if effective_budget and effective_budget.unit == "bytes"
                    else "characters"
                )
                usage_amount = result.context_summary.get(f"delivered_{usage_unit}")
                evidence = EvidenceDelivery(
                    intent=intent,
                    formed_at=owner.artifacts.clock.utc_now(),
                    snapshot=snapshot.ref,
                    captured_generation=snapshot.captured_generation,
                    materials=tuple(materials),
                    passages=tuple(passages),
                    coverage=result.coverage,
                    branch_coverage=result.branch_coverage,
                    continuation=tuple(continuation),
                    program=result.program,
                    program_run=result.program_run,
                    budget=effective_budget,
                    delivery_usage=DeliveryUsage(unit=usage_unit, amount=usage_amount)
                    if isinstance(usage_amount, int) and not isinstance(usage_amount, bool)
                    else None,
                    evaluation=TypeAdapter(dict[str, JsonValue]).validate_python(evaluation)
                    if isinstance(evaluation, dict)
                    else None,
                    issues=retrieval_issues,
                    diagnostics=result.diagnostics,
                )
            await call.events.emit(Progress(stage="retrieval", completed=1, total=1))
            evidence_ref = None
            if request.save or steward_policy in {"optional", "required"}:
                evidence_ref = await owner.evidence_adoption.save(
                    evidence, call, retain_snapshot=bool(anchor and anchor.view == "anchored")
                )
            reading = reading_ref = None
            issues = list(retrieval_issues)
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
                    arranged = await owner.steward.arrange(
                        ArrangeReading(
                            evidence=evidence_ref,
                            purpose=request.delivery.purpose,
                            detail=request.delivery.detail,
                            output_language=request.delivery.output_language,
                        ),
                        call,
                    )
                    reading, reading_ref = arranged.brief, arranged.reading_ref
                    issues.extend(arranged.issues)
                except EventDeliveryError:
                    raise
                except Exception as error:
                    if steward_policy == "required":
                        raise
                    issue = OperationIssue(
                        stage="steward",
                        code=error.code if isinstance(error, NousError) else "steward.failed",
                        message=str(error),
                        completed_refs=(evidence_ref.model_dump(mode="json"),),
                    )
                    issues.append(issue)
                    report_outcome("partial", issues=issues, completed_refs=(evidence_ref,))
                    await call.events.emit(Notice(issue=issue))
            return RetrievalOutcome(
                evidence=evidence,
                evidence_ref=evidence_ref,
                reading=reading,
                reading_ref=reading_ref,
                issues=tuple(issues),
            )
