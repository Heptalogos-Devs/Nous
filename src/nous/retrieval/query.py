from __future__ import annotations

import json
from typing import TYPE_CHECKING, Literal

import numpy as np
from nous_native import CancellationToken, lexical_scores
from nous_sdk.contracts import (
    CallContext,
    CognitiveRecord,
    DomainError,
    QueryCoverage,
    QueryHit,
    QueryRequest,
    QueryResult,
)
from nous_sdk.contracts.query_conditions import QueryEffectiveMode, QueryExecution
from nous_sdk.contracts.query_vcp import VcpResult
from nous_sdk.contracts.resources import ResourceRecord
from numpy.typing import NDArray
from pydantic import JsonValue

from .assets import record_domain, record_text
from .filters import authority
from .generations import published_all
from .native import run_native
from .planning import BoundPlan, bind, current_selection
from .policy import ResolvedPolicy, native_diagnostics
from .policy import resolve as resolve_policy
from .profiles import compile_profile
from .query_context import resolve_context
from .readout import visible_hits
from .resource_selection import prepare_sources
from .vcp_intent import concept_vectors, resolve_intent
from .vcp_thinking import resolve_thinking
from .work import QueryWork

if TYPE_CHECKING:
    from .service import RetrievalService


async def query(self: RetrievalService, ctx: CallContext, request: QueryRequest) -> QueryResult:
    request, context_plan = await resolve_context(self, ctx, request)
    bound, plan = bind(ctx, request)
    bound, profile_plan = await compile_profile(self.configuration, ctx, bound)
    if plan["sort"] is not None and (
        bound.rerank == "required" or bound.reranker in {"llm", "cross_encoder"}
    ):
        raise DomainError(
            "rerank_sort_conflict", "Explicit time sorting conflicts with relevance reranking"
        )
    plan["wave"] = profile_plan
    plan["query_context"] = context_plan
    policy = await resolve_policy(self.configuration, ctx)
    if bound.profile is not None:
        policy = ResolvedPolicy(bound.profile, "request", ctx.namespace_id)
    result = await execute(self, ctx, bound, plan, 0, QueryWork.from_budget(bound.budget), policy)
    if plan["diagnostics"] != "none":
        result = result.model_copy(update={"plan": {**result.plan, "policy": policy.diagnostics()}})
    result = result.model_copy(update={"expression": plan["canonical"]})
    return await self.finalize(
        ctx,
        bound.model_copy(
            update={
                "text": plan["canonical"],
                "materialize": bound.materialize or plan["materialize"],
            }
        ),
        result,
        explicit_sort=plan["sort"] is not None,
        current_selection=current_selection(bound, plan),
    )


async def execute(
    self: RetrievalService,
    ctx: CallContext,
    bound: QueryRequest,
    plan: BoundPlan,
    depth: int,
    work: QueryWork,
    policy: ResolvedPolicy,
) -> QueryResult:
    if depth >= bound.budget.depth:
        raise DomainError("query_budget_exceeded", "Query composition exceeds depth budget")
    lexical_enabled = bound.mode in {"lexical", "hybrid", "vcp"} and policy.value.lexical_weight > 0
    if depth and plan["root_controls"]:
        raise DomainError(
            "invalid_query",
            "History, cutoff, limit, effort, diagnostics and materialization belong at the query root",
        )
    bound = await prepare_sources(self, ctx, bound, plan)
    # Resolve addresses with the same authority visibility rules as canonical references.
    async with self.authority.read(ctx) as reader:
        if bound.reference:
            resolved = await self.references.resolve_in(
                reader, ctx, bound.reference, bound.as_of_commit, plan.get("asof_time")
            )
            bound = bound.model_copy(update={"reference": resolved.reference.reference.exact()})
            if resolved.reference.kind == "media":
                bound = bound.model_copy(
                    update={"domains": tuple(dict.fromkeys((*bound.domains, "media")))}
                )
        for cue in plan["selectors"]:
            if cue.get("selector") == "ref":
                locators: list[str] = []
                for locator in cue.get("locators", []):
                    resolved = await self.references.resolve_in(
                        reader, ctx, locator, bound.as_of_commit, plan.get("asof_time")
                    )
                    locators.append(resolved.reference.reference.exact())
                    bound = bound.model_copy(
                        update={
                            "domains": tuple(
                                dict.fromkeys((*bound.domains, resolved.reference.kind))
                            )
                        }
                    )
                cue["locators"] = locators
            elif cue.get("selector") == "resource":
                media = list(bound.media)
                for locator in cue.get("locators", []):
                    resolved = await self.references.resolve_in(
                        reader, ctx, locator, bound.as_of_commit, plan.get("asof_time")
                    )
                    if resolved.reference.kind != "media":
                        raise DomainError(
                            "invalid_query", "Resource cues require actual media references"
                        )
                    media.append(resolved.reference.reference)
                bound = bound.model_copy(update={"media": tuple(media)})
        plan["selectors"] = [cue for cue in plan["selectors"] if cue.get("selector") != "resource"]
        if bound.mode == "lexical" and (
            bound.media or any(not isinstance(part, str) for part in bound.parts)
        ):
            raise DomainError(
                "capability_unavailable", "Media query cues require dense, hybrid or VCP mode"
            )
    composition = plan.get("composition")
    if composition:
        if not composition["children"]:
            raise DomainError("invalid_query", "Query composition requires children")
        children: list[QueryResult] = []
        child_budget = bound.budget.candidates // len(composition["children"])
        if child_budget < 1:
            raise DomainError("query_budget_exceeded", "Composition exceeds candidate budget")
        for child in composition["children"]:
            child_bound, child_plan = bind(
                ctx,
                bound.model_copy(
                    update={
                        "text": child,
                        "limit": min(1000, child_budget),
                        "budget": bound.budget.model_copy(update={"candidates": child_budget}),
                    }
                ),
            )
            child_bound = child_bound.model_copy(
                update={
                    "budget": child_bound.budget.model_copy(
                        update={
                            "diagnostics_bytes": bound.budget.diagnostics_bytes
                            // len(composition["children"])
                        }
                    )
                }
            )
            child_bound = child_bound.model_copy(
                update={
                    "text": " ".join(value for value in (bound.text, child_bound.text) if value),
                    "domains": tuple(
                        value for value in child_bound.domains if value in bound.domains
                    ),
                    "kinds": tuple(
                        value
                        for value in child_bound.kinds
                        if not bound.kinds or value in bound.kinds
                    ),
                }
            )
            if bound.source_type and child_bound.source_type != bound.source_type:
                child_plan["no_matches"] = True
            if plan.get("no_matches"):
                child_plan["no_matches"] = True
            if bound.kinds and child_bound.kinds == ():
                child_bound = child_bound.model_copy(
                    update={
                        "domains": tuple(
                            value for value in child_bound.domains if value != "cognition"
                        )
                    }
                )
            child_plan["selectors"] = [*plan["selectors"], *child_plan["selectors"]]
            child_plan["exclude_sources"] = [
                *plan["exclude_sources"],
                *child_plan["exclude_sources"],
            ]
            child_plan["preferences"] = [*plan["preferences"], *child_plan["preferences"]]
            if "asof_time" in plan:
                child_plan["asof_time"] = min(
                    plan["asof_time"], child_plan.get("asof_time", plan["asof_time"])
                )
            children.append(
                await execute(self, ctx, child_bound, child_plan, depth + 1, work, policy)
            )
        keys: list[set[str]] = [
            {hit.object.reference.exact() for hit in child.hits} for child in children
        ]
        selected: set[str] = (
            (
                keys[0].union(*keys[1:])
                if composition["operation"] == "any"
                else keys[0].intersection(*keys[1:])
            )
            if keys
            else set[str]()
        )
        merged: dict[str, QueryHit] = {}
        for child in children:
            for rank, hit in enumerate(child.hits):
                key = hit.object.reference.exact()
                if key in selected:
                    previous = merged.get(key)
                    merged[key] = hit.model_copy(
                        update={
                            "score": (previous.score if previous else 0)
                            + 1 / (policy.value.rrf_k + rank + 1)
                        }
                    )
        ordered = self.order([(hit.object, hit.score) for hit in merged.values()], plan)
        modes: set[QueryEffectiveMode] = {
            child.execution.effective_mode for child in children if child.execution
        }
        return QueryResult(
            vcp=VcpResult(branches=tuple(child.vcp for child in children if child.vcp is not None))
            if any(child.vcp is not None for child in children)
            else None,
            hits=tuple(
                QueryHit(
                    object=record,
                    score=score,
                    source_roots=record.source_roots,
                    authority=authority(record),
                )
                for record, score in ordered[: bound.limit]
            ),
            coverage=QueryCoverage(
                indexed_through=min(
                    (child.coverage.indexed_through for child in children), default=0
                ),
                authority_commit_seq=max(
                    (child.coverage.authority_commit_seq for child in children), default=0
                ),
                complete=all(child.coverage.complete for child in children),
                generation="composition",
            ),
            plan={}
            if plan["diagnostics"] == "none"
            else json.loads(json.dumps({**plan, "branches": [child.plan for child in children]})),
            execution=QueryExecution(
                requested_mode=bound.mode,
                effective_mode=next(iter(modes)) if len(modes) == 1 else "mixed",
                candidate_budget=bound.budget.candidates,
                candidates_returned=len(ordered),
                eligible_objects=sum(
                    child.execution.eligible_objects for child in children if child.execution
                ),
                missing_vectors=sum(
                    child.execution.missing_vectors for child in children if child.execution
                ),
                candidate_limited=any(
                    child.execution and child.execution.candidate_limited for child in children
                ),
                degraded=any(child.execution and child.execution.degraded for child in children),
                reasons=tuple(
                    dict.fromkeys(
                        reason
                        for child in children
                        if child.execution
                        for reason in child.execution.reasons
                    )
                ),
            ),
        )
    work.branch()
    view = await self.eligible(ctx, bound, plan)
    index_generations = await published_all(self, ctx)
    selection = self.selectors(view.records, view.catalog, plan, view.sources)
    records, direct = selection.records, selection.direct
    if not lexical_enabled and bound.mode == "lexical" and bound.text and not direct:
        raise DomainError(
            "capability_unavailable",
            "The Wave lexical channel is disabled",
            stage="lexical",
        )
    intent = resolve_intent(bound, plan, selection, view.catalog, view.sources, policy, work)
    if bound.mode == "vcp" and intent.thinking:
        direct = False
    terms = tuple(
        dict.fromkeys((*selection.terms, *intent.terms, *(cue.text for cue in bound.vcp.concepts)))
    )
    if bound.mode != "lexical" and terms:
        bound = bound.model_copy(
            update={
                "text": " ".join((bound.text, *terms)).strip(),
                "parts": (*bound.parts, *terms) if bound.parts else (),
            }
        )
    if plan.get("no_matches"):
        records = []
    records = [record for record in records if record_domain(record) in bound.domains]
    if bound.kinds:
        records = [
            record
            for record in records
            if not isinstance(record, CognitiveRecord) or record.reference.kind in bound.kinds
        ]
    if bound.reference:
        records = [
            record
            for record in records
            if record.reference.exact() == bound.reference
            or f"{record.reference.kind}:{record.reference.id}" == bound.reference
        ]
        direct = not (bound.mode == "vcp" and intent.thinking)
    missing = 0
    requested_mode = bound.mode
    effective_mode = bound.mode
    reasons: list[str] = []
    if bound.current == "prefer" and any(
        isinstance(record, ResourceRecord)
        and record.reference.exact() not in plan.get("checked_resources", [])
        for record in records
    ):
        reasons.append("external_snapshot_stale")
    if plan.get("explore") and bound.mode != "vcp":
        if bound.capabilities.exploration == "required":
            raise DomainError("capability_unavailable", "$explore requires VCP mode")
        reasons.append("exploration_requires_vcp")
    query_vector: NDArray[np.float32] | None = None
    space: str | None = None
    native_info: dict[str, JsonValue] = {}
    vcp_result: VcpResult | None = None
    ghost_vectors: NDArray[np.float32] | None = None
    core_references = intent.core
    matching_vectors: set[str] = set()
    has_query_input = bool(
        bound.text.strip()
        or bound.media
        or any(not isinstance(part, str) or part.strip() for part in bound.parts)
    )
    if bound.mode != "lexical" and not direct and records and has_query_input:
        try:
            batch = await self.query_embedding(
                ctx,
                bound,
                source_ids=tuple(sorted(set((*selection.source_material_ids, *intent.source_ids)))),
                concepts=tuple(concept.text for concept in intent.concepts),
            )
        except DomainError as error:
            if bound.capabilities.semantic != "optional" or error.code not in {
                "model_unavailable",
                "model_not_configured",
                "embedding_unavailable",
                "capability_unavailable",
                "configuration_missing",
            }:
                raise
            effective_mode = "lexical" if lexical_enabled else bound.mode
            reasons.append(error.code)
            batch = None
    else:
        batch = None
    if batch is not None:
        if (
            len(batch.output.vectors) != 1 + len(intent.concepts)
            or len(batch.output.vectors[0]) != batch.output.dimensions
            or not np.isfinite(batch.output.vectors[0]).all()
        ):
            raise DomainError(
                "invalid_embedding", "Query embedding has invalid dimensions or coordinates"
            )
        space = batch.output.space
        by_ref = {
            row.reference: row
            for row in view.vectors
            if row.space == space
            and row.model == batch.output.model
            and row.dimensions == batch.output.dimensions
            and len(row.vector) == batch.output.dimensions
            and np.isfinite(row.vector).all()
            and any(row.vector)
        }
        matching_vectors = set(by_ref)
        eligible = [record for record in records if record.reference.exact() in by_ref]
        missing = len(records) - len(eligible)
        if missing and bound.capabilities.require_complete_vectors:
            raise DomainError(
                "missing_vectors",
                "Semantic query requires matching vectors for all selected candidates",
                missing=missing,
                space=space,
            )
        if not eligible and bound.capabilities.semantic == "required":
            raise DomainError(
                "capability_unavailable",
                "No selected candidate has a vector in the query model space",
                space=space,
            )
        if missing:
            reasons.append("partial_semantic_coverage")
        if bound.mode != "hybrid":
            records = eligible
        query_vector = np.ascontiguousarray(batch.output.vectors[0], dtype=np.float32)
        if bound.mode == "vcp":
            semantic_generation = index_generations.get(space)
            available = set(semantic_generation.graph_keys) if semantic_generation else set()
            available.intersection_update(record.reference.exact() for record in view.catalog)
            if not available:
                if bound.capabilities.exploration == "required":
                    raise DomainError(
                        "vcp_assets_unavailable",
                        "VCP requires a published authorized graph generation",
                    )
                reasons.append("vcp_assets_unavailable")
                effective_mode = "hybrid" if lexical_enabled else "dense"
            else:
                core_references = intent.graph_core(available, bound, reasons)
                ghost_vectors, concept_results = concept_vectors(intent, batch)
                vcp_result = VcpResult(core=core_references, concepts=concept_results)
    if intent.thinking and (effective_mode != "vcp" or not records):
        vcp_result = VcpResult(
            core=core_references,
            thinking=await resolve_thinking(
                self,
                ctx,
                {
                    "status": "unavailable",
                    "reason": "no_candidates" if not records else "vcp_mode_unavailable",
                },
                view.catalog,
                records,
                bound,
                reasons,
            ),
        )
    if records and (bound.text or bound.mode != "lexical") and not direct:
        selected_records = {record.reference.exact(): record for record in records}
        channel_hits: dict[str, list[tuple[str, float]]] = {}
        watermarks: list[int] = []
        generations: list[str] = []
        stages: list[JsonValue] = []
        channels: list[Literal["lexical", "dense", "vcp"]] = []
        if effective_mode in {"lexical", "hybrid"} and lexical_enabled:
            channels.append("lexical")
        if query_vector is not None and effective_mode != "lexical":
            channels.append(
                "vcp" if effective_mode == "vcp" and vcp_result is not None else "dense"
            )
        for channel in channels:
            asset = index_generations.get(None if channel == "lexical" else space)
            if asset is None:
                if channel != "lexical" and bound.capabilities.semantic == "required":
                    raise DomainError(
                        "index_not_ready",
                        "No published semantic generation is available",
                        space=space,
                    )
                reasons.append(channel + "_index_not_ready")
                stages.append({"stage": channel, "status": "unavailable", "candidates": 0})
                continue
            snapshot = asset.snapshot
            refs = snapshot.references
            allowed = np.ascontiguousarray(
                [
                    index
                    for index, ref in enumerate(refs)
                    if ref in selected_records and (channel == "lexical" or ref in matching_vectors)
                ],
                dtype=np.uint64,
            )
            allowed_graph = (
                np.ascontiguousarray(
                    [
                        index
                        for index, ref in enumerate(asset.graph_keys)
                        if ref in matching_vectors
                        and ref in {record.reference.exact() for record in view.catalog}
                    ],
                    dtype=np.uint64,
                )
                if channel == "vcp"
                else None
            )
            token = CancellationToken()
            result = await run_native(
                "query",
                token,
                snapshot.query,
                cancel=token,
                mode=channel,
                text=bound.text,
                vectors=query_vector if channel != "lexical" else None,
                allowed_rows=allowed,
                allowed_graph_nodes=allowed_graph,
                options_json=json.dumps(
                    intent.options(
                        policy,
                        bound.budget.candidates,
                        plan["diagnostics"],
                        core_references,
                        strict=bound.capability_policy == "strict",
                    )
                    if channel == "vcp"
                    else policy.options(bound.budget.candidates, plan["diagnostics"])
                ),
                ghost_vectors=ghost_vectors if channel == "vcp" else None,
            )
            if channel == "vcp":
                availability = (
                    json.loads(result.diagnostics_json)
                    .get("vcp", {})
                    .get("scoped_stage_availability", {})
                )
                reasons.extend(
                    stage + "_unavailable_in_scope"
                    for stage, status in availability.items()
                    if status == "unavailable_in_scope"
                )
            channel_hits[channel] = [
                (refs[int(index)], float(score))
                for index, score in zip(result.row_ids, result.scores, strict=True)
            ]
            watermarks.append(asset.watermark)
            generations.append(result.generation)
            stages.append(
                {
                    "stage": channel,
                    "status": "completed",
                    "candidates": len(result.row_ids),
                    "indexed_through": asset.watermark,
                    "generation": result.generation,
                }
            )
            native_info[channel] = native_diagnostics(
                result.diagnostics_json, plan["diagnostics"], bound.budget.diagnostics_bytes
            )
            if channel == "vcp" and vcp_result is not None and intent.thinking:
                thinking = json.loads(result.diagnostics_json)["vcp"]["thinking"]
                vcp_result = vcp_result.model_copy(
                    update={
                        "thinking": await resolve_thinking(
                            self, ctx, thinking, view.catalog, records, bound, reasons
                        )
                    }
                )
        context = (
            bound.context
            if bound.context is not None and not isinstance(bound.context, str)
            else None
        )
        if context is not None:
            cue_refs = set(
                (
                    *context.anchors,
                    *context.entities,
                    *(reference.exact() for reference in context.attachments),
                )
            )
            cues = [
                row
                for row in view.vectors
                if row.reference in cue_refs
                and len(row.vector) == row.dimensions
                and np.isfinite(row.vector).all()
                and any(row.vector)
            ]
            if effective_mode != "lexical":
                per_cue = max(1, bound.budget.candidates // max(1, len(cues)))
                for cue_index, cue in enumerate(cues):
                    asset = index_generations.get(cue.space)
                    if asset is None:
                        continue
                    refs = asset.snapshot.references
                    available = {
                        row.reference
                        for row in view.vectors
                        if row.space == cue.space
                        and row.model == cue.model
                        and row.dimensions == cue.dimensions
                    }
                    allowed = np.ascontiguousarray(
                        [
                            index
                            for index, ref in enumerate(refs)
                            if ref in selected_records and ref in available
                        ],
                        dtype=np.uint64,
                    )
                    mode = (
                        "vcp"
                        if not has_query_input
                        and effective_mode == "vcp"
                        and "vcp" in asset.snapshot.capabilities
                        else "dense"
                    )
                    token = CancellationToken()
                    cue_result = await run_native(
                        "context_cue",
                        token,
                        asset.snapshot.query,
                        cancel=token,
                        mode=mode,
                        text="",
                        vectors=np.ascontiguousarray(cue.vector, dtype=np.float32),
                        allowed_rows=allowed,
                        allowed_graph_nodes=np.ascontiguousarray(
                            [
                                index
                                for index, ref in enumerate(asset.graph_keys)
                                if ref in {record.reference.exact() for record in view.catalog}
                            ],
                            dtype=np.uint64,
                        )
                        if mode == "vcp"
                        else None,
                        options_json=json.dumps(
                            intent.options(
                                policy,
                                per_cue,
                                plan["diagnostics"],
                                core_references,
                                strict=bound.capability_policy == "strict",
                            )
                            if mode == "vcp"
                            else policy.options(per_cue, plan["diagnostics"])
                        ),
                    )
                    channel_hits[f"context:{cue_index}"] = [
                        (refs[int(index)], float(score))
                        for index, score in zip(cue_result.row_ids, cue_result.scores, strict=True)
                    ]
                    watermarks.append(asset.watermark)
                    generations.append(cue_result.generation)
                    stages.append(
                        {
                            "stage": "context_dense" if mode == "dense" else "context_vcp",
                            "source": cue.reference,
                            "generation": cue_result.generation,
                            "indexed_through": asset.watermark,
                            "candidates": len(cue_result.row_ids),
                        }
                    )
            if effective_mode in {"lexical", "hybrid"} and lexical_enabled:
                terms = [
                    value
                    for value in (*context.entities, context.goal)
                    if value and ":" not in value
                ]
                asset = index_generations.get(None)
                if asset is not None and terms:
                    refs = asset.snapshot.references
                    allowed = np.ascontiguousarray(
                        [index for index, ref in enumerate(refs) if ref in selected_records],
                        dtype=np.uint64,
                    )
                    token = CancellationToken()
                    cue_result = await run_native(
                        "context_lexical",
                        token,
                        asset.snapshot.query,
                        cancel=token,
                        mode="lexical",
                        text=" ".join(terms),
                        allowed_rows=allowed,
                        options_json=json.dumps(
                            policy.options(bound.budget.candidates, plan["diagnostics"])
                        ),
                    )
                    channel_hits["context_lexical"] = [
                        (refs[int(index)], float(score))
                        for index, score in zip(cue_result.row_ids, cue_result.scores, strict=True)
                    ]
                    watermarks.append(asset.watermark)
                    generations.append(cue_result.generation)
                    stages.append(
                        {
                            "stage": "context_lexical",
                            "generation": cue_result.generation,
                            "indexed_through": asset.watermark,
                            "candidates": len(cue_result.row_ids),
                        }
                    )
        indexed_through = min(watermarks, default=0)
        lexical_refs = set()
        lexical_asset = index_generations.get(None)
        if lexical_asset is not None:
            lexical_refs = set(lexical_asset.snapshot.references)
        delta = [record for record in records if record.reference.exact() not in lexical_refs]
        if delta and effective_mode in {"lexical", "hybrid"} and lexical_enabled:
            if len(delta) > bound.budget.candidates:
                reasons.append("index_backlog_exceeds_delta_budget")
            delta = delta[: bound.budget.candidates]
            delta_scores = await run_native(
                "lexical_delta",
                None,
                lexical_scores,
                bound.text,
                [record_text(record) for record in delta],
            )
            scored = [
                (record.reference.exact(), float(score))
                for record, score in zip(delta, delta_scores, strict=True)
                if score is not None
            ]
            scored = sorted(
                (item for item in scored if item[1] > 0), key=lambda item: (-item[1], item[0])
            )
            channel_hits.setdefault("lexical", []).extend(scored)
            stages.append(
                {"stage": "lexical_delta", "status": "completed", "candidates": len(scored)}
            )
        if bound.min_commit_seq is not None and indexed_through < bound.min_commit_seq:
            raise DomainError(
                "index_watermark_unavailable",
                "Published indexes have not reached the requested watermark",
                indexed_through=indexed_through,
                required=bound.min_commit_seq,
            )
        scores: dict[str, float] = {}
        if len(channel_hits) == 1:
            scores.update(next(iter(channel_hits.values())))
        else:
            for channel, hits_for_channel in channel_hits.items():
                weight = (
                    policy.value.lexical_weight
                    if channel == "lexical"
                    else policy.value.dense_weight
                )
                for rank, (ref, _) in enumerate(hits_for_channel, 1):
                    scores[ref] = scores.get(ref, 0.0) + weight / (policy.value.rrf_k + rank)
        ranked = [(selected_records[ref], score) for ref, score in scores.items()]
        generation = ",".join(dict.fromkeys(generations)) or "pending"
        native_info["stages"] = stages
    else:
        ranked, generation = [(record, 1.0) for record in records], "authority"
        indexed_through = view.commit_seq
    ranked = self.order(ranked, plan)
    ranked = ranked[: bound.budget.candidates]
    hits = await visible_hits(
        self, ctx, ranked, bound.limit, current_selection=current_selection(bound, plan)
    )
    complete = (
        missing == 0
        and indexed_through >= max((record.commit_seq for record in records), default=0)
        and (bound.min_commit_seq is None or view.commit_seq >= bound.min_commit_seq)
        and plan.get("resource_refresh_complete", True)
    )
    checked = set(plan.get("checked_resources", []))
    hits = [
        hit.model_copy(update={"authority": "external_current_authority"})
        if isinstance(hit.object, ResourceRecord) and hit.object.reference.exact() in checked
        else hit
        for hit in hits
    ]
    return QueryResult(
        vcp=vcp_result,
        hits=tuple(hits),
        coverage=QueryCoverage(
            indexed_through=indexed_through,
            authority_commit_seq=view.commit_seq,
            complete=complete,
            generation=generation,
        ),
        execution=QueryExecution(
            requested_mode=requested_mode,
            effective_mode=effective_mode,
            candidate_budget=bound.budget.candidates,
            candidates_returned=len(ranked),
            eligible_objects=len(records),
            missing_vectors=missing,
            candidate_limited=len(records) > bound.budget.candidates,
            degraded=bool(reasons),
            reasons=tuple(reasons),
        ),
        plan={}
        if plan["diagnostics"] == "none"
        else json.loads(
            json.dumps(
                {
                    **plan,
                    "eligible_objects": len(records),
                    "missing_vectors": missing,
                    "native": native_info,
                }
            )
        ),
    )
