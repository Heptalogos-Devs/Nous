from __future__ import annotations

from typing import TYPE_CHECKING

import numpy as np
from nous_wave.contracts import (
    AssociationBody,
    CognitiveRecord,
    ConceptBody,
)
from numpy.typing import NDArray

from nous.cognition.projection import references_in

from .assets import (
    GraphAssets,
    RetrievalRecord,
    VectorAsset,
)

if TYPE_CHECKING:
    from .service import RetrievalService


def graph(
    self: RetrievalService,
    catalog: list[RetrievalRecord],
    records: list[RetrievalRecord],
    assets: dict[str, VectorAsset],
    dimensions: int,
    active_roots: dict[str, str],
) -> GraphAssets:
    concepts = [
        record
        for record in catalog
        if isinstance(record, CognitiveRecord)
        and isinstance(record.body, ConceptBody)
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
        for ref in references_in(record.body if isinstance(record, CognitiveRecord) else None):
            if ref.id in tag_rows and not (
                isinstance(record, CognitiveRecord) and isinstance(record.body, AssociationBody)
            ):
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
        if isinstance(record, CognitiveRecord) and isinstance(record.body, AssociationBody):
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
