from __future__ import annotations

import asyncio
import json
from hashlib import sha256
from typing import TYPE_CHECKING

import numpy as np
from nous_native import IndexStore
from nous_wave.contracts import (
    CallContext,
    CognitiveRecord,
    DomainError,
    QueryCoverage,
    QueryHit,
    QueryRequest,
    QueryResult,
)
from numpy.typing import NDArray

from .assets import (
    GraphAssets,
    record_text,
)
from .planning import bind

if TYPE_CHECKING:
    from .service import RetrievalService


async def query(self: RetrievalService, ctx: CallContext, request: QueryRequest) -> QueryResult:
    bound, plan = bind(ctx, request)
    # Resolve addresses with the same authority visibility rules as canonical references.
    async with self.authority.read(ctx) as reader:
        if bound.reference:
            resolved = await self.references.resolve_in(
                reader, ctx, bound.reference, bound.as_of_commit
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
                        reader, ctx, locator, bound.as_of_commit
                    )
                    locators.append(resolved.reference.reference.exact())
                    if resolved.reference.kind == "media":
                        bound = bound.model_copy(
                            update={"domains": tuple(dict.fromkeys((*bound.domains, "media")))}
                        )
                cue["locators"] = locators
    composition = plan.get("composition")
    if composition:
        children = [
            await self.query(ctx, bound.model_copy(update={"text": child, "limit": 1000}))
            for child in composition["children"]
        ]
        keys = [{hit.object.reference.exact() for hit in child.hits} for child in children]
        selected = (
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
                        update={"score": (previous.score if previous else 0) + 1 / (61 + rank)}
                    )
        ordered = self.order([(hit.object, hit.score) for hit in merged.values()], plan)
        return QueryResult(
            hits=tuple(
                QueryHit(object=record, score=score, source_roots=record.source_roots)
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
            plan=json.loads(json.dumps(plan)),
        )
    view = await self.eligible(ctx, bound, plan)
    records, direct = self.selectors(view.records, plan, view.sources)
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
    query_vector: NDArray[np.float32] | None = None
    dense: NDArray[np.float32] | None = None
    graph: GraphAssets | None = None
    space: str | None = None
    if bound.mode != "lexical" and not direct and records:
        batch = await self.query_embedding(ctx, bound)
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
        records = eligible
        dense = np.ascontiguousarray(
            [by_ref[record.reference.exact()].vector for record in records], dtype=np.float32
        )
        query_vector = np.ascontiguousarray(batch.vectors[0], dtype=np.float32)
        if bound.mode == "vcp":
            graph = self.graph(view.records, records, by_ref, batch.dimensions, view.active_roots)
            if not graph.tags:
                raise DomainError(
                    "vcp_assets_unavailable",
                    "VCP requires live, authorized Concept vectors in the query model space",
                )
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
                store = IndexStore(str(self.index_path / key))
                snapshot = await asyncio.to_thread(
                    store.build,
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
                self.stores[key], self.snapshots[key] = store, snapshot
            result = await asyncio.to_thread(
                snapshot.query,
                mode=bound.mode,
                text=bound.text,
                vectors=query_vector,
                allowed_rows=np.arange(len(records), dtype=np.uint64),
                allowed_graph_nodes=np.arange(len(graph.tags), dtype=np.uint64) if graph else None,
                options_json=json.dumps({"limit": min(1000, max(bound.limit * 4, bound.limit))}),
            )
        ranked = [
            (records[int(index)], float(score))
            for index, score in zip(result.row_ids, result.scores, strict=True)
        ]
        generation = result.generation
    else:
        ranked, generation = [(record, 1.0) for record in records], "authority"
    ranked = self.order(ranked, plan)
    hits: list[QueryHit] = []
    async with self.authority.read(ctx) as current:
        roots = await self.cognition.active_roots_in(current, ctx)
        for record, score in ranked:
            if await self.cognition.visible_in(current, ctx, record, roots):
                hits.append(QueryHit(object=record, score=score, source_roots=record.source_roots))
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
        plan={}
        if plan["diagnostics"] == "none"
        else json.loads(
            json.dumps({**plan, "eligible_objects": len(records), "missing_vectors": missing})
        ),
    )
