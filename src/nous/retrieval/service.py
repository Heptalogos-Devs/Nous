"""Authority-scoped plans feed immutable batches into native retrieval."""

from __future__ import annotations

import asyncio
import json
import shutil
from dataclasses import dataclass
from hashlib import sha256
from pathlib import Path

import numpy as np
from nous_native import IndexStore, Snapshot
from nous_wave.contracts import (
    AssociationBody,
    CallContext,
    CognitiveRecord,
    ConceptBody,
    DomainError,
    EmbeddingBatch,
    EpisodeBody,
    MediaEmbeddingInput,
    MediaRecord,
    MediaReference,
    QueryCoverage,
    QueryHit,
    QueryRequest,
    QueryResult,
)
from numpy.typing import NDArray
from pydantic import BaseModel, TypeAdapter
from sqlalchemy import select

from nous.cognition.models import ModelServices
from nous.cognition.service import CognitionService, body_text, references_in, revisions, vectors
from nous.identity.service import require_subject_in
from nous.material.service import SourceRecord, observations, require_observation_in
from nous.platform.persistence import Authority

from .media import embedding_input, media_records_in
from .planning import BoundPlan, bind, time_matches


class VectorAsset(BaseModel):
    reference: str
    space: str
    model: str
    dimensions: int
    vector: list[float]
    call_id: str | None
    commit_seq: int


_VECTOR_ROWS = TypeAdapter(list[VectorAsset])

RetrievalRecord = CognitiveRecord | MediaRecord

def record_text(record: RetrievalRecord) -> str:
    return body_text(record.body) if isinstance(record, CognitiveRecord) else record.text


@dataclass(frozen=True)
class CandidateView:
    records: list[RetrievalRecord]
    vectors: list[VectorAsset]
    commit_seq: int
    sources: dict[str, SourceRecord]
    active_roots: dict[str, str]


@dataclass(frozen=True)
class GraphAssets:
    tags: list[dict[str, str]]
    vectors: NDArray[np.float32]
    member_indptr: NDArray[np.uint64]
    member_indices: NDArray[np.uint64]
    edge_indptr: NDArray[np.uint64]
    edge_indices: NDArray[np.uint64]
    edge_weights: NDArray[np.float64]
    edge_polarities: NDArray[np.int8]
    source_references: list[str]
    candidate_source_indptr: NDArray[np.uint64]
    candidate_source_indices: NDArray[np.uint64]
    graph_source_indptr: NDArray[np.uint64]
    graph_source_indices: NDArray[np.uint64]


class RetrievalService:
    def __init__(
        self,
        authority: Authority,
        cognition: CognitionService,
        models: ModelServices,
        index_path: str | Path,
    ) -> None:
        self.authority, self.cognition, self.models = authority, cognition, models
        self.index_path = Path(index_path)
        self.index_path.mkdir(parents=True, exist_ok=True)
        self._snapshots: dict[str, Snapshot] = {}
        self._stores: dict[str, IndexStore] = {}
        self._lock = asyncio.Lock()

    async def close(self) -> None:
        async with self._lock:
            for store in self._stores.values():
                store.close()
            self._snapshots.clear()
            self._stores.clear()

    async def invalidate(self) -> None:
        await self.close()
        await asyncio.to_thread(shutil.rmtree, self.index_path, True)
        self.index_path.mkdir(parents=True, exist_ok=True)

    async def _eligible(
        self, ctx: CallContext, request: QueryRequest, plan: BoundPlan
    ) -> CandidateView:
        async with self.authority.read(ctx) as view:
            await require_subject_in(view, ctx)
            statement = select(revisions.c.record).where(
                revisions.c.namespace_id == ctx.namespace_id,
                revisions.c.subject_id == ctx.subject_id,
            )
            if request.as_of_commit is not None:
                statement = statement.where(revisions.c.commit_seq <= request.as_of_commit)
            rows = [
                CognitiveRecord.model_validate(row)
                for row in (await view.connection.execute(statement)).scalars()
            ]
            if "asof_time" in plan:
                rows = [row for row in rows if row.times.recorded.coordinate <= plan["asof_time"]]
            exact = bool(request.reference and "@" in request.reference) or any(
                cue.get("selector") == "ref"
                and any("@" in locator for locator in cue.get("locators", []))
                for cue in plan["selectors"]
            )
            if not request.history and not exact:
                heads: dict[str, CognitiveRecord] = {}
                for row in rows:
                    if (
                        row.reference.id not in heads
                        or heads[row.reference.id].reference.revision < row.reference.revision
                    ):
                        heads[row.reference.id] = row
                rows = list(heads.values())
            candidates: list[RetrievalRecord] = [*rows, *(await media_records_in(view, ctx))]
            roots = await self.cognition.active_roots_in(view, ctx)
            allowed: list[RetrievalRecord] = []
            source_rows: dict[str, SourceRecord] = {
                row["id"]: row["record"]
                for row in (
                    await view.connection.execute(
                        select(observations.c.id, observations.c.record).where(
                            observations.c.namespace_id == ctx.namespace_id,
                            observations.c.subject_id == ctx.subject_id,
                        )
                    )
                ).mappings()
            }
            for row in candidates:
                if row.deleted and not request.include_deleted:
                    continue
                if (
                    not request.include_inactive
                    and not exact
                    and isinstance(row, CognitiveRecord)
                    and isinstance(row.body, (ConceptBody, EpisodeBody))
                    and row.body.state != "active"
                ):
                    continue
                if isinstance(row, CognitiveRecord) and row.lifecycle is not None and row.lifecycle.activation != "active" and not request.include_inactive and not exact:
                    continue
                if not time_matches(row, request.times):
                    continue
                if not await self.cognition.visible_in(view, ctx, row, roots):
                    continue
                live = [oid for oid in row.source_roots if oid in roots]
                if request.source_ids is not None and not all(
                    oid in request.source_ids or roots[oid] in request.source_ids for oid in live
                ):
                    continue
                if request.source_type and not any(
                    source_rows[oid]["source_type"] == request.source_type for oid in live
                ):
                    continue
                if any(source_rows[oid]["source_type"] in plan["exclude_sources"] for oid in live):
                    continue
                allowed.append(row)
            deduped: dict[str, RetrievalRecord] = {}
            for item in allowed:
                previous = deduped.get(item.reference.exact())
                if previous is None or item.commit_seq > previous.commit_seq:
                    deduped[item.reference.exact()] = item
            allowed = list(deduped.values())
            assets = _VECTOR_ROWS.validate_python(
                (await view.connection.execute(select(vectors))).mappings().all()
            )
            return CandidateView(allowed, assets, view.commit_seq, source_rows, roots)

    def _selectors(
        self, records: list[RetrievalRecord], plan: BoundPlan, sources: dict[str, SourceRecord]
    ) -> tuple[list[RetrievalRecord], bool]:
        candidates, direct = records, False
        for cue in plan["selectors"]:
            if cue["kind"] != "selector":
                continue
            selector, locators = cue.get("selector", ""), cue.get("locators", [])
            matched: list[set[str]] = []
            for locator in locators:
                if selector == "ref":
                    values = [
                        record
                        for record in records
                        if record.reference.exact() == locator
                        or f"{record.reference.kind}:{record.reference.id}" == locator
                    ]
                    if "@" not in locator and values:
                        values = [max(values, key=lambda record: record.reference.revision if isinstance(record,CognitiveRecord) else record.commit_seq)]
                    direct = True
                elif selector == "object":
                    values = [
                        record
                        for record in records
                        if any(
                            oid in sources and locator in sources[oid]["origin"].values()
                            for oid in record.source_roots
                        )
                    ]
                elif selector == "resource":
                    values = [
                        record
                        for record in records
                        if any(
                            oid in sources
                            and sources[oid]["material_id"] == locator.removeprefix("material:")
                            for oid in record.source_roots
                        )
                    ]
                else:
                    kind = {"tag": "concept", "entity": "entity", "schema": "schema"}.get(selector)
                    values = [
                        record
                        for record in records
                        if record.reference.kind == kind
                        and locator.casefold()
                        in [
                            str(
                                getattr(getattr(record, "body", None), "label", getattr(getattr(record, "body", None), "title", ""))
                            ).casefold(),
                            *(
                                str(alias).casefold()
                                for alias in getattr(getattr(record, "body", None), "aliases", ())
                            ),
                            record.reference.exact().casefold(),
                        ]
                    ]
                    if len(values) > 1:
                        raise DomainError(
                            "ambiguous_name",
                            "Selector matches multiple cognitive objects",
                            candidates=[value.reference.model_dump() for value in values],
                        )
                    if values:
                        target_ids = {value.reference.id for value in values}
                        explicit_members = {
                            ref.id
                            for value in values
                            if isinstance(value,CognitiveRecord) and isinstance(value.body, ConceptBody)
                            for ref in value.body.members
                        }
                        values = [
                            record
                            for record in records
                            if record.reference.id in target_ids | explicit_members
                            or any(
                                ref.id in target_ids
                                for ref in references_in(record.body.model_dump(mode="json") if isinstance(record,CognitiveRecord) and record.body is not None else None)
                            )
                            or any(
                                link.reference and link.reference.id in target_ids
                                for link in (record.evidence if isinstance(record,CognitiveRecord) else ())
                            )
                        ]
                if not values:
                    raise DomainError(
                        "selector_not_found", f"No accessible object matches {locator}"
                    )
                matched.append({value.reference.exact() for value in values})
            selected = matched[0].intersection(*matched[1:]) if matched else set[str]()
            candidates = [record for record in candidates if record.reference.exact() in selected]
        return candidates, direct

    def _graph(
        self,
        catalog: list[RetrievalRecord],
        records: list[RetrievalRecord],
        assets: dict[str, VectorAsset],
        dimensions: int,
        active_roots: dict[str, str],
    ) -> GraphAssets:
        concepts = [
            record
            for record in catalog
            if isinstance(record,CognitiveRecord) and isinstance(record.body, ConceptBody)
            and record.body.canonical is None
            and record.body.state == "active"
            and record.reference.exact() in assets
        ]
        tags = [
            {"key": record.reference.exact(), "label": record.body.label}
            for record in concepts
            if isinstance(record.body, ConceptBody)
        ]
        tag_rows = {record.reference.id: index for index, record in enumerate(concepts)}
        memberships: dict[str, set[int]] = {}
        for tag, record in enumerate(concepts):
            if isinstance(record.body, ConceptBody):
                for member in record.body.members:
                    memberships.setdefault(member.id, set()).add(tag)
        member_indptr: list[int] = [0]
        member_indices: list[int] = []
        for record in records:
            nodes = set(memberships.get(record.reference.id, ()))
            for ref in references_in(record.body.model_dump(mode="json") if isinstance(record,CognitiveRecord) and record.body is not None else None):
                if ref.id in tag_rows and not (isinstance(record,CognitiveRecord) and isinstance(record.body, AssociationBody)):
                    nodes.add(tag_rows[ref.id])
            if record.reference.id in tag_rows:
                nodes.add(tag_rows[record.reference.id])
            member_indices.extend(sorted(nodes))
            member_indptr.append(len(member_indices))

        def material_roots(record: RetrievalRecord) -> tuple[str, ...]:
            return tuple(
                sorted(
                    {
                        active_roots[oid]
                        for oid in record.support_roots or record.source_roots
                        if oid in active_roots
                    }
                )
            )

        adjacency: list[list[tuple[int, float, int, tuple[str, ...]]]] = [[] for _ in concepts]
        for record in catalog:
            if isinstance(record,CognitiveRecord) and isinstance(record.body, AssociationBody):
                body = record.body
                if body.from_ref.id in tag_rows and body.to_ref.id in tag_rows:
                    adjacency[tag_rows[body.from_ref.id]].append(
                        (
                            tag_rows[body.to_ref.id],
                            1.0,
                            1 if body.polarity == "positive" else -1,
                            material_roots(record),
                        )
                    )
        edge_indptr: list[int] = [0]
        edge_indices: list[int] = []
        edge_weights: list[float] = []
        edge_polarities: list[int] = []
        edge_source_roots: list[tuple[str, ...]] = []
        for neighbors in adjacency:
            for target, weight, polarity, sources in sorted(neighbors):
                edge_indices.append(target)
                edge_weights.append(weight)
                edge_polarities.append(polarity)
                edge_source_roots.append(sources)
            edge_indptr.append(len(edge_indices))
        tag_vectors = np.ascontiguousarray(
            [assets[record.reference.exact()].vector for record in concepts], dtype=np.float32
        )
        if not concepts:
            tag_vectors = np.empty((0, dimensions), dtype=np.float32)
        candidate_roots = [material_roots(record) for record in records]
        source_references = sorted(
            {root for source_set in [*candidate_roots, *edge_source_roots] for root in source_set}
        )
        source_rows = {root: index for index, root in enumerate(source_references)}

        def source_csr(
            source_sets: list[tuple[str, ...]],
        ) -> tuple[NDArray[np.uint64], NDArray[np.uint64]]:
            offsets: list[int] = [0]
            indices: list[int] = []
            for source_set in source_sets:
                indices.extend(source_rows[root] for root in source_set)
                offsets.append(len(indices))
            return np.asarray(offsets, dtype=np.uint64), np.asarray(indices, dtype=np.uint64)

        candidate_source_indptr, candidate_source_indices = source_csr(candidate_roots)
        graph_source_indptr, graph_source_indices = source_csr(edge_source_roots)
        return GraphAssets(
            tags,
            tag_vectors,
            np.asarray(member_indptr, dtype=np.uint64),
            np.asarray(member_indices, dtype=np.uint64),
            np.asarray(edge_indptr, dtype=np.uint64),
            np.asarray(edge_indices, dtype=np.uint64),
            np.asarray(edge_weights, dtype=np.float64),
            np.asarray(edge_polarities, dtype=np.int8),
            source_references,
            candidate_source_indptr,
            candidate_source_indices,
            graph_source_indptr,
            graph_source_indices,
        )

    async def query(self, ctx: CallContext, request: QueryRequest) -> QueryResult:
        bound, plan = bind(ctx, request)
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
            ordered = self._order([(hit.object, hit.score) for hit in merged.values()], plan)
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
        view = await self._eligible(ctx, bound, plan)
        records, direct = self._selectors(view.records, plan, view.sources)
        records = [record for record in records if ("cognition" if isinstance(record,CognitiveRecord) else "media") in bound.domains]
        if bound.kinds:
            records = [record for record in records if not isinstance(record,CognitiveRecord) or record.reference.kind in bound.kinds]
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
            batch = await self._query_embedding(ctx, bound)
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
                graph = self._graph(
                    view.records, records, by_ref, batch.dimensions, view.active_roots
                )
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
            async with self._lock:
                snapshot = self._snapshots.get(key)
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
                    self._stores[key], self._snapshots[key] = store, snapshot
                result = await asyncio.to_thread(
                    snapshot.query,
                    mode=bound.mode,
                    text=bound.text,
                    vectors=query_vector,
                    allowed_rows=np.arange(len(records), dtype=np.uint64),
                    allowed_graph_nodes=np.arange(len(graph.tags), dtype=np.uint64)
                    if graph
                    else None,
                    options_json=json.dumps(
                        {"limit": min(1000, max(bound.limit * 4, bound.limit))}
                    ),
                )
            ranked = [
                (records[int(index)], float(score))
                for index, score in zip(result.row_ids, result.scores, strict=True)
            ]
            generation = result.generation
        else:
            ranked, generation = [(record, 1.0) for record in records], "authority"
        ranked = self._order(ranked, plan)
        hits: list[QueryHit] = []
        async with self.authority.read(ctx) as current:
            roots = await self.cognition.active_roots_in(current, ctx)
            for record, score in ranked:
                if await self.cognition.visible_in(current, ctx, record, roots):
                    hits.append(
                        QueryHit(object=record, score=score, source_roots=record.source_roots)
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
            plan={}
            if plan["diagnostics"] == "none"
            else json.loads(
                json.dumps({**plan, "eligible_objects": len(records), "missing_vectors": missing})
            ),
        )

    def _order(
        self, ranked: list[tuple[RetrievalRecord, float]], plan: BoundPlan
    ) -> list[tuple[RetrievalRecord, float]]:
        for preference in plan["preferences"]:
            text = preference.get("text")
            if text:
                direction = -1 if preference["negative"] else 1
                ranked = [
                    (
                        record,
                        score
                        + direction * 0.1 * (text.casefold() in record_text(record).casefold()),
                    )
                    for record, score in ranked
                ]
        ranked.sort(key=lambda item: (-item[1], item[0].reference.exact()))
        sort = plan["sort"]
        if sort:

            def key(item: tuple[RetrievalRecord, float]) -> tuple[bool, int, float, str]:
                value = getattr(item[0].times, sort["axis"])
                coordinate = (
                    value.start
                    if sort["axis"] == "validity" and value
                    else value.coordinate
                    if value
                    else None
                )
                known = (
                    value is not None and value.domain == sort["domain"] and coordinate is not None
                )
                return (
                    not known,
                    -int(coordinate)
                    if known and coordinate is not None and sort["direction"] == "desc"
                    else int(coordinate)
                    if known and coordinate is not None
                    else 0,
                    -item[1],
                    item[0].reference.exact(),
                )

            ranked.sort(key=key)
        return ranked

    async def _query_embedding(self, ctx: CallContext, request: QueryRequest) -> EmbeddingBatch:
        parts = request.parts or (request.text, *request.media)
        if not any(isinstance(part, MediaReference) for part in parts):
            return await self.models.embed(ctx.model_copy(update={"request_id": f"{ctx.request_id}:query"}), (request.text,))
        async with self.authority.read(ctx) as view:
            available = await media_records_in(view, ctx)
        converted: list[str | MediaEmbeddingInput] = []
        source_ids: set[str] = set()
        for part in parts:
            if isinstance(part, str):
                converted.append(part)
                continue
            matched = [record for record in available if record.reference.exact() == part.exact() and (part.observation_id is None or part.observation_id in record.source_roots)]
            if not matched:
                raise DomainError("source_unavailable", "Query media is outside the current source scope")
            record = max(matched, key=lambda item: item.commit_seq)
            converted.append(embedding_input(record))
            source_ids.add(record.material_id)
        return await self.models.embed_media(ctx.model_copy(update={"request_id": f"{ctx.request_id}:query-media"}), (tuple(converted),), source_ids=tuple(sorted(source_ids)))

    async def embed_media(self, ctx: CallContext, reference: MediaReference, operation_id: str):
        receipt = await self.authority.receipt(ctx, operation_id, operation_kind="media.embed", scope=ctx.require_subject(), payload=reference)
        if receipt is not None:
            return receipt.result
        async with self.authority.read(ctx) as view:
            candidates = [record for record in await media_records_in(view, ctx) if record.reference.exact() == reference.exact() and (reference.observation_id is None or reference.observation_id in record.source_roots)]
        if not candidates:
            raise DomainError("source_unavailable", "Media embedding source is unavailable")
        record = max(candidates, key=lambda item: item.commit_seq)
        batch = await self.models.embed_media(ctx.model_copy(update={"request_id": operation_id}), ((embedding_input(record),),), source_ids=(record.material_id,))
        if len(batch.vectors) != 1 or len(batch.vectors[0]) != batch.dimensions or not np.isfinite(batch.vectors[0]).all():
            raise DomainError("invalid_embedding", "Media embedding has invalid count, dimensions, or coordinates")
        async with self.authority.write(ctx, operation_id, operation_kind="media.embed", scope=ctx.require_subject(), payload=reference) as tx:
            if tx.replayed:
                return tx.replay_result
            await require_observation_in(tx, ctx, record.source_roots[0], region_id=record.region.id if record.region else None)
            existing = (await tx.connection.execute(select(vectors.c.model, vectors.c.dimensions).where(vectors.c.reference == reference.exact(), vectors.c.space == batch.space))).first()
            if existing is not None and (existing.model != batch.model or existing.dimensions != batch.dimensions):
                raise DomainError("embedding_space_conflict", "A media space cannot mix model identities or dimensions")
            if existing is None:
                await tx.connection.execute(vectors.insert().values(reference=reference.exact(), space=batch.space, model=batch.model, dimensions=batch.dimensions, vector=list(batch.vectors[0]), call_id=batch.call_id, commit_seq=tx.commit_seq))
            return tx.result({"reference": reference.model_dump(mode="json"), "space": batch.space, "model": batch.model, "dimensions": batch.dimensions, "call_ids": list(batch.call_ids or ((batch.call_id,) if batch.call_id else ())), "commit_seq": tx.commit_seq})
