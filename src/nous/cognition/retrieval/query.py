"""Bind a user's query, deliver exact evidence and optionally arrange it."""

from __future__ import annotations

from contextlib import AsyncExitStack
from typing import TYPE_CHECKING

from pydantic import JsonValue, TypeAdapter

from nous.application.contracts import CallContext, EventDeliveryError, Notice, Progress
from nous.cognition.contracts import NamespaceRef
from nous.cognition.material_contracts import MaterialSelection
from nous.cognition.steward.contracts import ArrangeReading
from nous.common.errors import NousError
from nous.common.operations import OperationIssue, exception_context, report_outcome
from nous.content.contracts import (
    ResultOutputRef,
    ResultRef,
)
from nous.wave.contracts import (
    DeliveredText,
    DeliveryStatus,
    ProgramRunFacts,
    SelectedContext,
    WaveQuery,
    WaveSearchResult,
)
from nous.wave.serialization import content_signature

from .binding import QueryBinder, merge_controls
from .context import ContextPreparer
from .contracts import (
    BoundQueryControls,
    CognitiveGroupContext,
    ContinuationLink,
    DeliveryUsage,
    EvidenceDelivery,
    PassageEvidence,
    QueryAtom,
    QueryControls,
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
        budget = request.delivery.budget or self.owner.wave.delivery.default_budget()
        bodies = tuple(view.body for view in limited if view.body is not None)
        text_artifacts = {
            body.artifact.id
            for body, is_text in zip(
                bodies, await self.owner.artifacts.text_capabilities(bodies), strict=True
            )
            if is_text
        }
        counter = await self.owner.wave.delivery.selection_budget(
            budget,
            tuple(
                view.body
                for view in limited
                if view.body is not None and view.body.artifact.id in text_artifacts
            ),
        )
        materials: list[RetrievedMaterial] = []
        passages: list[PassageEvidence] = []
        continuation: list[ContinuationLink] = []
        for index, view in enumerate(limited, 1):
            keys: tuple[str, ...] = ()
            status: DeliveryStatus = "no_candidates"
            if view.body is not None and view.body.artifact.id in text_artifacts:
                selected, truncated = await self.owner.execution.blocking(
                    counter.fit_prefix, view.body
                )
                if selected:
                    content = await self.owner.artifacts.read_selections(selected)
                    key = f"E{len(passages) + 1}"
                    passages.append(
                        PassageEvidence(
                            key=key,
                            material=view.ref,
                            fragments=tuple(
                                DeliveredText(content=part.content, text=part.text)
                                for part in content
                            ),
                            scored_ranges=tuple(part.content for part in content),
                            selection_reason="explicit_reference",
                        )
                    )
                    keys = (key,)
                    status = "selected"
                    if truncated:
                        continuation.append(
                            ContinuationLink(target=view.ref, relation="read_remaining")
                        )
                elif truncated:
                    status = "budget_exhausted"
                else:
                    status = "overlap"
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
        intent = QueryIntent(
            original_text=request.text,
            aspects=request.aspects,
            constraints=BoundQueryControls(controls=controls, namespace=namespace),
            bound_materials=refs,
        )
        facts: dict[str, JsonValue] = {
            "name": "direct",
            "definition_signature": content_signature({"name": "direct"}),
            "requested_program": None,
            "selection": "explicit",
            "requirements": [],
            "nodes": [],
            "outputs": {},
            "options": {},
            "budget": budget.model_dump(mode="json") if budget else None,
            "space_id": None,
            "space_signature": None,
            "space_definition": None,
        }
        program_run = ProgramRunFacts.model_validate(
            {**facts, "signature": content_signature(facts)}
        )
        program_run = self.owner.wave.delivery.budget_facts(program_run, counter)
        return EvidenceDelivery(
            intent=intent,
            formed_at=self.owner.artifacts.clock.utc_now(),
            snapshot=None,
            captured_generation=None,
            materials=tuple(materials),
            passages=tuple(passages),
            continuation=tuple(continuation),
            program="direct",
            program_run=program_run,
            budget=budget,
            delivery_usage=DeliveryUsage(
                unit=budget.unit if budget else "characters",
                amount=counter.used,
            ),
        )

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
            prior_ref = (await owner.results.get(prior_ref)).ref
        if anchor and anchor.view == "anchored":
            snapshot_ref = anchor.snapshot
            if snapshot_ref is None and prior_ref is not None:
                artifact = await owner.results.resolve_output(
                    ResultOutputRef(result=prior_ref, output="evidence")
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
        if direct:
            namespace = await owner.scope.resolve_for(request.namespace)
            async with owner.database.read() as tx:
                ns = await owner.scope.get_in(tx, namespace.id)
        else:
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
        async with AsyncExitStack() as stack:
            if snapshot is not None:
                await stack.enter_async_context(owner.wave.snapshots.hold_reference(snapshot.ref))
            retrieval_issues: tuple[OperationIssue, ...] = ()
            if direct:
                evidence = await self._direct(request, root_controls, ns.ref)
            else:
                assert snapshot is not None
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
                (
                    bound,
                    bound_materials,
                    bound_concepts,
                    bound_branches,
                ) = await self.binder.bind_expression(
                    expression, request.controls, ns.ref, snapshot.ref.identifier, call, root=True
                )
                result = await owner.wave.query(
                    WaveQuery(
                        snapshot=snapshot.ref.identifier,
                        text=request.text
                        if request.expression is not None and request.text.strip()
                        else self.binder.expression_text(expression),
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
                )
                selected_context = TypeAdapter(tuple[SelectedContext, ...]).validate_python(
                    result.context_summary.get("selected_context", [])
                )
                used: list[UsedContext] = []
                for item in selected_context:
                    source = sources_by_key.get(item.key)
                    if source is None and item.content is not None:
                        source = MaterialSelection(
                            material=item.content.artifact, selections=(item.content,)
                        )
                    used.append(
                        UsedContext(
                            key=item.key,
                            source=source,
                            text=item.text,
                            role=item.role,
                            stage=item.stage,
                            parent_key=item.parent_key or item.key,
                            contribution=item.contribution,
                        )
                    )
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
                usage_unit = effective_budget.unit if effective_budget else "characters"
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
                latest_evidence = evidence_ref
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
                    latest_evidence = arranged.evidence_ref
                    reading, reading_ref = arranged.brief, arranged.reading_ref
                    issues.extend(arranged.issues)
                except EventDeliveryError:
                    raise
                except Exception as error:
                    if steward_policy == "required":
                        raise
                    completed = exception_context(error).get("evidence_ref")
                    if completed is not None:
                        latest_evidence = ResultRef.model_validate(completed)
                    issue = OperationIssue(
                        stage="steward",
                        code=error.code if isinstance(error, NousError) else "steward.failed",
                        message=str(error),
                        completed_refs=(latest_evidence.model_dump(mode="json"),),
                    )
                    issues.append(issue)
                    report_outcome("partial", issues=issues, completed_refs=(latest_evidence,))
                    await call.events.emit(Notice(issue=issue))
                if latest_evidence != evidence_ref:
                    artifact = await owner.results.resolve_output(
                        ResultOutputRef(result=latest_evidence, output="evidence")
                    )
                    evidence = EvidenceDelivery.model_validate_json(
                        await owner.artifacts.read_text(artifact)
                    )
                    evidence_ref = latest_evidence
            return RetrievalOutcome(
                evidence=evidence,
                evidence_ref=evidence_ref,
                reading=reading,
                reading_ref=reading_ref,
                issues=tuple(issues),
            )
