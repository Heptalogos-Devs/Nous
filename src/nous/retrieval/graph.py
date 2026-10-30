from __future__ import annotations

from typing import TYPE_CHECKING

import numpy as np
from nous_sdk.contracts import (
    AssociationBody,
    CognitiveRecord,
    CognitiveRef,
    ConceptBody,
    EntityBody,
)
from numpy.typing import NDArray

from nous.cognition.lifecycle import selectable
from nous.cognition.projection import references_in

from .assets import (
    GraphAssets,
    RetrievalRecord,
    VectorAsset,
)

if TYPE_CHECKING:
    from .service import RetrievalService


def node_label(record: CognitiveRecord) -> str:
    if isinstance(record.body, (ConceptBody, EntityBody)) and record.body.label.strip():
        return record.body.label
    return record.addresses.exact if record.addresses else record.reference.exact()


def graph(
    self: RetrievalService,
    catalog: list[RetrievalRecord],
    records: list[RetrievalRecord],
    assets: dict[str, VectorAsset],
    dimensions: int,
    active_roots: dict[str, str],
    *,
    core_references: tuple[CognitiveRef, ...] = (),
    include_inactive: bool = False,
) -> GraphAssets:
    # Numerical graph nodes retain canonical typed exact identities. Concept is
    # one domain; Entity and explicit relation endpoints are not synthesized Tags.
    cognitive = {
        record.reference.exact(): record
        for record in catalog
        if isinstance(record, CognitiveRecord)
        and selectable(record, include_inactive=include_inactive)
    }
    associations = [
        record for record in cognitive.values() if isinstance(record.body, AssociationBody)
    ]
    endpoint_keys = {
        reference.exact()
        for record in associations
        if isinstance(record.body, AssociationBody)
        for reference in (record.body.from_ref, record.body.to_ref)
    }
    endpoint_keys.update(ref.exact() for ref in core_references)
    nodes = [
        record
        for record in cognitive.values()
        if (
            isinstance(record.body, EntityBody)
            or record.reference.exact() in endpoint_keys
            or isinstance(record.body, ConceptBody)
        )
        and (
            not isinstance(record.body, ConceptBody)
            or (record.body.canonical is None and record.body.state == "active")
        )
        and record.reference.exact() in assets
    ]
    nodes.sort(key=lambda record: record.reference.exact())
    tags = [{"key": record.reference.exact(), "label": node_label(record)} for record in nodes]
    tag_rows = {record.reference.exact(): index for index, record in enumerate(nodes)}
    memberships: dict[str, set[int]] = {}
    for tag, record in enumerate(nodes):
        if isinstance(record.body, ConceptBody):
            for member in record.body.members:
                memberships.setdefault(member.exact(), set()).add(tag)
        elif isinstance(record.body, EntityBody):
            for mention in record.body.mentions:
                memberships.setdefault(mention.reference.exact(), set()).add(tag)
    member_indptr: list[int] = [0]
    member_indices: list[int] = []
    for record in records:
        members = set(memberships.get(record.reference.exact(), ()))
        for ref in references_in(record.body if isinstance(record, CognitiveRecord) else None):
            if ref.exact() in tag_rows:
                members.add(tag_rows[ref.exact()])
        if record.reference.exact() in tag_rows:
            members.add(tag_rows[record.reference.exact()])
        # A relation has explicit direction. Unordered memberships otherwise use
        # stable identity order; labels and candidate order never create edges.
        ordered: list[int] = []
        if isinstance(record, CognitiveRecord) and isinstance(record.body, AssociationBody):
            ordered = list(
                dict.fromkeys(
                    tag_rows[ref.exact()]
                    for ref in (record.body.from_ref, record.body.to_ref)
                    if ref.exact() in tag_rows
                )
            )
        member_indices.extend((*ordered, *sorted(members.difference(ordered))))
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

    adjacency: list[list[tuple[int, float, int, tuple[str, ...]]]] = [[] for _ in nodes]
    omitted_endpoints = 0
    omitted_vectors = 0
    omitted_inactive = 0
    omitted_sources = 0
    for record in associations:
        if isinstance(record.body, AssociationBody):
            body = record.body
            endpoints = (body.from_ref.exact(), body.to_ref.exact())
            if not all(key in cognitive for key in endpoints):
                omitted_endpoints += 1
                continue
            if not all(key in assets for key in endpoints):
                omitted_vectors += 1
                continue
            if not all(key in tag_rows for key in endpoints):
                omitted_inactive += 1
                continue
            sources = material_roots(record)
            if not sources:
                omitted_sources += 1
                continue
            if body.from_ref.exact() in tag_rows and body.to_ref.exact() in tag_rows:
                adjacency[tag_rows[body.from_ref.exact()]].append(
                    (
                        tag_rows[body.to_ref.exact()],
                        body.weight if body.weight is not None else 1.0,
                        1 if body.polarity == "positive" else -1,
                        sources,
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
        [assets[record.reference.exact()].vector for record in nodes], dtype=np.float32
    )
    if not nodes:
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
        {
            "nodes": len(nodes),
            "concept_nodes": sum(isinstance(record.body, ConceptBody) for record in nodes),
            "entity_nodes": sum(isinstance(record.body, EntityBody) for record in nodes),
            "other_endpoint_nodes": sum(
                not isinstance(record.body, (ConceptBody, EntityBody)) for record in nodes
            ),
            "association_records": len(associations),
            "directed_edges": len(edge_indices),
            "positive_edges": edge_polarities.count(1),
            "negative_edges": edge_polarities.count(-1),
            "omitted_endpoint_edges": omitted_endpoints,
            "omitted_vector_edges": omitted_vectors,
            "omitted_inactive_edges": omitted_inactive,
            "omitted_source_edges": omitted_sources,
        },
    )
