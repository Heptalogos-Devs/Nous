from __future__ import annotations

import json
from hashlib import sha256
from typing import TYPE_CHECKING

import numpy as np
from nous_native import CancellationToken, IndexStore, NativeError
from nous_wave.contracts import (
    CallContext,
    CognitiveRecord,
    DomainError,
    QueryCoverage,
    QueryHit,
    QueryRequest,
    QueryResult,
)
from nous_wave.contracts.query_conditions import QueryEffectiveMode, QueryExecution
from numpy.typing import NDArray
from pydantic import JsonValue

from .assets import (
    GraphAssets,
    record_text,
)
from .filters import authority
from .media import source_material_ids_in
from .native import failure, run_native
from .planning import BoundPlan, bind
from .policy import ResolvedPolicy, native_diagnostics
from .policy import resolve as resolve_policy

if TYPE_CHECKING:
    from .service import RetrievalService


async def query(self: RetrievalService, ctx: CallContext, request: QueryRequest) -> QueryResult:
    bound, plan = bind(ctx, request)
    policy = await resolve_policy(self.configuration, ctx)
    result = await execute(self, ctx, bound, plan, 0, [bound.budget.branches], policy)
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
    )


async def execute(
    self: RetrievalService,
    ctx: CallContext,
    bound: QueryRequest,
    plan: BoundPlan,
    depth: int,
    branches: list[int],
    policy: ResolvedPolicy,
) -> QueryResult:
    if depth >= bound.budget.depth:
        raise DomainError("query_budget_exceeded", "Query composition exceeds depth budget")
    if depth and plan["root_controls"]:
        raise DomainError(
            "invalid_query",
            "History, cutoff, limit, effort, diagnostics and materialization belong at the query root",
        )
    if bound.current == "required" or bound.filters.authority in {
        "external_current_authority",
        "resource_descriptor",
    }:
        raise DomainError(
            "capability_unavailable",
            "External authority requires a resource directory and an adapter with access and version checks",
            capability="external_current_authority",
            current=bound.current,
            authority=bound.filters.authority,
        )
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
                    if resolved.reference.kind == "media":
                        bound = bound.model_copy(
                            update={"domains": tuple(dict.fromkeys((*bound.domains, "media")))}
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
                await execute(self, ctx, child_bound, child_plan, depth + 1, branches, policy)
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
    branches[0] -= 1
    if branches[0] < 0:
        raise DomainError("query_budget_exceeded", "Query exceeds branch budget")
    view = await self.eligible(ctx, bound, plan)
    selection = self.selectors(view.records, view.catalog, plan, view.sources)
    records, direct = selection.records, selection.direct
    if bound.mode != "lexical" and selection.terms:
        bound = bound.model_copy(
            update={
                "text": " ".join((bound.text, *selection.terms)).strip(),
                "parts": (*bound.parts, *selection.terms) if bound.parts else (),
            }
        )
    if plan.get("no_matches"):
        records = []
    records = [
        record
        for record in records
        if ("cognition" if isinstance(record, CognitiveRecord) else "media") in bound.domains
    ]
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
        direct = True
    missing = 0
    requested_mode = bound.mode
    reasons: list[str] = []
    if bound.current == "prefer":
        reasons.append("external_freshness_unchecked")
    if plan.get("explore") and bound.mode != "vcp":
        if bound.capabilities.exploration == "required":
            raise DomainError("capability_unavailable", "$explore requires VCP mode")
        reasons.append("exploration_requires_vcp")
    query_vector: NDArray[np.float32] | None = None
    dense: NDArray[np.float32] | None = None
    graph: GraphAssets | None = None
    space: str | None = None
    native_info: dict[str, JsonValue] = {}
    if bound.mode != "lexical" and not direct and records:
        try:
            batch = await self.query_embedding(ctx, bound, source_ids=selection.source_material_ids)
        except DomainError as error:
            if bound.capabilities.semantic != "optional" or error.code not in {
                "model_unavailable",
                "model_not_configured",
                "embedding_unavailable",
                "capability_unavailable",
                "configuration_missing",
            }:
                raise
            bound = bound.model_copy(update={"mode": "lexical"})
            reasons.append(error.code)
            batch = None
    else:
        batch = None
    if batch is not None:
        if (
            len(batch.vectors) != 1
            or len(batch.vectors[0]) != batch.dimensions
            or not np.isfinite(batch.vectors[0]).all()
        ):
            raise DomainError(
                "invalid_embedding", "Query embedding has invalid dimensions or coordinates"
            )
        space = batch.space
        by_ref = {
            row.reference: row
            for row in view.vectors
            if row.space == space
            and row.model == batch.model
            and row.dimensions == batch.dimensions
        }
        eligible = [record for record in records if record.reference.exact() in by_ref]
        missing = len(records) - len(eligible)
        if missing and bound.capabilities.require_complete_vectors:
            raise DomainError(
                "missing_vectors",
                "Semantic query requires matching vectors for all selected candidates",
                missing=missing,
                space=space,
            )
        if not eligible and bound.capabilities.semantic == "required" and bound.mode != "hybrid":
            raise DomainError(
                "capability_unavailable",
                "No selected candidate has a vector in the query model space",
                space=space,
            )
        if missing and bound.mode == "hybrid":
            reasons.append("missing_vectors_lexical_fallback")
            bound = bound.model_copy(update={"mode": "lexical"})
            eligible = records
        elif missing:
            reasons.append("missing_vectors_omitted")
            if not eligible:
                bound = bound.model_copy(update={"mode": "lexical"})
                eligible = records
                reasons.append("semantic_unavailable_lexical_fallback")
        records = eligible
        dense = (
            np.ascontiguousarray(
                [by_ref[record.reference.exact()].vector for record in records], dtype=np.float32
            )
            if bound.mode != "lexical"
            else None
        )
        query_vector = np.ascontiguousarray(batch.vectors[0], dtype=np.float32)
        if bound.mode == "vcp":
            graph = self.graph(view.catalog, records, by_ref, batch.dimensions, view.active_roots)
            if not graph.tags:
                if bound.capabilities.exploration == "required":
                    raise DomainError(
                        "vcp_assets_unavailable",
                        "VCP requires live, authorized Concept vectors in the query model space",
                    )
                reasons.append("vcp_assets_unavailable")
                bound = bound.model_copy(update={"mode": "hybrid"})
                graph = None
    if records and (bound.text or bound.mode != "lexical") and not direct:
        refs, texts = (
            [record.reference.exact() for record in records],
            [record_text(record) for record in records],
        )
        identity = [
            refs,
            texts,
            space,
            dense.tolist() if dense is not None else None,
            [
                graph.tags,
                graph.vectors.tolist(),
                graph.member_indptr.tolist(),
                graph.member_indices.tolist(),
                graph.edge_indptr.tolist(),
                graph.edge_indices.tolist(),
                graph.edge_weights.tolist(),
                graph.edge_polarities.tolist(),
                graph.source_references,
                graph.candidate_source_indptr.tolist(),
                graph.candidate_source_indices.tolist(),
                graph.graph_source_indptr.tolist(),
                graph.graph_source_indices.tolist(),
            ]
            if graph
            else None,
        ]
        key = sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
        async with self.lock:
            snapshot = self.snapshots.get(key)
            if snapshot is None:
                try:
                    store = IndexStore(str(self.index_path / key))
                except NativeError as error:
                    raise failure(error, "open") from error
                token = CancellationToken()
                try:
                    snapshot = await run_native(
                        "build",
                        token,
                        store.build,
                        cancel=token,
                        references=refs,
                        texts=texts,
                        assets_json=json.dumps(
                            {"space": space, "tags": graph.tags}
                            if graph
                            else {"space": space}
                            if space
                            else {}
                        ),
                        vectors=dense,
                        tag_vectors=graph.vectors if graph else None,
                        tag_indptr=graph.member_indptr if graph else None,
                        tag_indices=graph.member_indices if graph else None,
                        graph_indptr=graph.edge_indptr if graph else None,
                        graph_indices=graph.edge_indices if graph else None,
                        graph_weights=graph.edge_weights if graph else None,
                        graph_polarities=graph.edge_polarities if graph else None,
                        source_references=graph.source_references if graph else None,
                        candidate_source_indptr=graph.candidate_source_indptr if graph else None,
                        candidate_source_indices=graph.candidate_source_indices if graph else None,
                        graph_source_indptr=graph.graph_source_indptr if graph else None,
                        graph_source_indices=graph.graph_source_indices if graph else None,
                    )
                except BaseException:
                    await run_native("close_failed_build", None, store.close)
                    raise
                self.stores[key], self.snapshots[key] = store, snapshot
            token = CancellationToken()
            result = await run_native(
                "query",
                token,
                snapshot.query,
                cancel=token,
                mode=bound.mode,
                text=bound.text,
                vectors=query_vector,
                allowed_rows=np.arange(len(records), dtype=np.uint64),
                allowed_graph_nodes=np.arange(len(graph.tags), dtype=np.uint64) if graph else None,
                options_json=json.dumps(
                    policy.options(bound.budget.candidates, plan["diagnostics"])
                ),
            )
        native_info = native_diagnostics(
            result.diagnostics_json, plan["diagnostics"], bound.budget.diagnostics_bytes
        )
        ranked = [
            (records[int(index)], float(score))
            for index, score in zip(result.row_ids, result.scores, strict=True)
        ]
        generation = result.generation
    else:
        ranked, generation = [(record, 1.0) for record in records], "authority"
    ranked = self.order(ranked, plan)
    ranked = ranked[: bound.budget.candidates]
    hits: list[QueryHit] = []
    async with self.authority.read(ctx) as current:
        roots = await self.cognition.active_roots_in(current, ctx)
        for record, score in ranked:
            if isinstance(record, CognitiveRecord):
                visible = await self.cognition.visible_in(current, ctx, record, roots)
            else:
                try:
                    await source_material_ids_in(
                        current, ctx, record.reference, self.materials, self.derivations
                    )
                    visible = True
                except DomainError as error:
                    if error.code not in {
                        "source_unavailable",
                        "region_not_found",
                        "representation_not_found",
                        "derived_region_not_found",
                    }:
                        raise
                    visible = False
            if visible:
                hits.append(
                    QueryHit(
                        object=record,
                        score=score,
                        source_roots=record.source_roots,
                        authority=authority(record),
                    )
                )
            if len(hits) >= bound.limit:
                break
    complete = missing == 0 and (
        bound.min_commit_seq is None or view.commit_seq >= bound.min_commit_seq
    )
    return QueryResult(
        hits=tuple(hits),
        coverage=QueryCoverage(
            indexed_through=view.commit_seq if not missing else 0,
            authority_commit_seq=view.commit_seq,
            complete=complete,
            generation=generation,
        ),
        execution=QueryExecution(
            requested_mode=requested_mode,
            effective_mode=bound.mode,
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
