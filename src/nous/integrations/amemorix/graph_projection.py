"""Project the canonical graph directory into the established dashboard contract."""

from __future__ import annotations

from nous_sdk.contracts import (
    AssociationBody,
    CognitiveRecord,
    DomainError,
    EntityBody,
    MemoryBody,
)
from nous_sdk.contracts.graph_management import GraphSnapshot
from pydantic import JsonValue

from nous.cognition.graph_state import resolve_entity

from .helpers import json_object


def paragraphs(record: CognitiveRecord, snapshot: GraphSnapshot) -> list[CognitiveRecord]:
    refs = {
        link.reference.exact()
        for link in record.evidence
        if link.reference and link.reference.kind == "memory"
    }
    if isinstance(record.body, EntityBody):
        refs.update(
            mention.reference.exact()
            for mention in record.body.mentions
            if mention.reference.kind == "memory"
        )
    return [value for value in snapshot.paragraphs if value.reference.exact() in refs]


def edge(row: CognitiveRecord, known: dict[str, CognitiveRecord]) -> dict[str, JsonValue]:
    assert isinstance(row.body, AssociationBody)
    left, right = known.get(row.body.from_ref.id), known.get(row.body.to_ref.id)
    if (
        left is None
        or right is None
        or not isinstance(left.body, EntityBody)
        or not isinstance(right.body, EntityBody)
    ):
        raise DomainError(
            "graph_endpoint_unavailable", "The relation endpoint is outside the current graph"
        )
    value = json_object(
        {
            "hash": row.reference.id,
            "subject": left.body.label,
            "predicate": row.body.relation,
            "object": right.body.label,
            "text": left.body.label + " " + row.body.relation + " " + right.body.label,
            "reference": row.reference,
            "weight": row.body.weight,
            "source_roots": row.source_roots,
        }
    )
    value["confidence"] = row.body.epistemic.confidence if row.body.epistemic else None
    return value


def paragraph(row: CognitiveRecord, snapshot: GraphSnapshot) -> dict[str, JsonValue]:
    assert isinstance(row.body, MemoryBody)
    entities = [
        value
        for value in snapshot.records
        if isinstance(value.body, EntityBody)
        and any(item.reference == row.reference for item in value.body.mentions)
    ]
    associations = [
        value
        for value in snapshot.records
        if isinstance(value.body, AssociationBody)
        and any(link.reference == row.reference for link in value.evidence)
    ]
    return json_object(
        {
            "hash": row.reference.id,
            "entity_count": len(entities),
            "relation_count": len(associations),
            "entities": [
                value.body.label for value in entities if isinstance(value.body, EntityBody)
            ],
            "relations": [value.reference.id for value in associations],
            "content": row.body.text,
            "preview": row.body.text[:220],
            "source": row.body.metadata.get("source"),
            "created_at": row.times.formation.coordinate / 1000
            if row.times.formation.domain == "unix_ms"
            else None,
            "updated_at": row.times.recorded.coordinate / 1000
            if row.times.recorded.domain == "unix_ms"
            else None,
            "reference": row.reference,
            "source_roots": row.source_roots,
        }
    )


def graph_view(snapshot: GraphSnapshot, limit: int) -> dict[str, JsonValue]:
    entities = [row for row in snapshot.records if isinstance(row.body, EntityBody)]
    selected = entities[:limit]
    known = {row.reference.id: row for row in entities}
    ids = {row.reference.id for row in selected}
    labels = [row.body.label for row in entities if isinstance(row.body, EntityBody)]
    if len(set(labels)) != len(labels):
        raise DomainError(
            "ambiguous_graph_names",
            "The legacy graph view has duplicate names; use the exact Native graph directory",
        )
    groups: dict[tuple[str, str], list[CognitiveRecord]] = {}
    for row in snapshot.records:
        if (
            isinstance(row.body, AssociationBody)
            and row.body.from_ref.id in known
            and row.body.to_ref.id in known
        ):
            groups.setdefault((row.body.from_ref.id, row.body.to_ref.id), []).append(row)
    nodes = [
        json_object(
            {
                "id": row.body.label,
                "name": row.body.label,
                "attributes": {
                    "reference": row.reference,
                    "aliases": row.body.effective_aliases,
                    "profile_text": row.body.profile_text,
                    "entity_type": row.body.entity_type,
                },
            }
        )
        for row in selected
        if isinstance(row.body, EntityBody)
    ]
    edges: list[dict[str, JsonValue]] = []
    for (left, right), values in groups.items():
        if left not in ids or right not in ids:
            continue
        payloads = [edge(row, known) for row in values]
        weights = [
            row.body.weight
            for row in values
            if isinstance(row.body, AssociationBody) and row.body.weight is not None
        ]
        predicates = list(dict.fromkeys(str(value["predicate"]) for value in payloads))
        edges.append(
            json_object(
                {
                    "source": payloads[0]["subject"],
                    "target": payloads[0]["object"],
                    "weight": sum(weights) / len(weights) if weights else 1.0,
                    "recorded_weight": sum(weights) / len(weights) if weights else None,
                    "weight_basis": "recorded" if weights else "neutral_prior",
                    "relation_hashes": [row.reference.id for row in values],
                    "predicates": predicates,
                    "label": " / ".join(predicates),
                }
            )
        )
    return json_object(
        {
            "success": True,
            "nodes": nodes,
            "edges": edges,
            "total_nodes": len(entities),
            "total_edges": len(groups),
            "watermark": snapshot.watermark,
        }
    )


def details(
    snapshot: GraphSnapshot,
    *,
    node: str | None = None,
    source: str | None = None,
    target: str | None = None,
    relation_limit: int = 20,
    paragraph_limit: int = 20,
    evidence_node_limit: int = 80,
) -> dict[str, JsonValue]:
    known = {row.reference.id: row for row in snapshot.records if isinstance(row.body, EntityBody)}
    left = resolve_entity(snapshot.records, node or source or "")
    right = resolve_entity(snapshot.records, target) if target else None
    if left is None or (target and right is None):
        raise DomainError("entity_not_found", "Graph detail selector has no accessible Entity")
    related = [
        row
        for row in snapshot.records
        if isinstance(row.body, AssociationBody)
        and (
            left.reference.id in {row.body.from_ref.id, row.body.to_ref.id}
            if node
            else row.body.from_ref.id == left.reference.id
            and right is not None
            and row.body.to_ref.id == right.reference.id
        )
    ][:relation_limit]
    if not node and not related:
        raise DomainError(
            "association_not_found", "No current Association joins the selected endpoints"
        )
    supporting = {
        value.reference.exact(): value
        for row in (left, *related)
        for value in paragraphs(row, snapshot)
    }
    selected = list(supporting.values())[:paragraph_limit]
    relations: list[dict[str, JsonValue]] = []
    for row in related:
        value = edge(row, known)
        refs = paragraphs(row, snapshot)
        value.update(
            json_object(
                {
                    "paragraph_count": len(refs),
                    "paragraph_hashes": [ref.reference.id for ref in refs],
                    "source_paragraph": next(
                        (
                            link.reference.id
                            for link in row.evidence
                            if link.role == "source_paragraph" and link.reference
                        ),
                        None,
                    ),
                }
            )
        )
        relations.append(value)
    evidence_nodes: list[dict[str, JsonValue]] = []
    evidence_edges: list[dict[str, JsonValue]] = []
    for row in (
        left,
        *(
            value
            for value in known.values()
            if any(
                isinstance(relation.body, AssociationBody)
                and value.reference.id in {relation.body.from_ref.id, relation.body.to_ref.id}
                for relation in related
            )
        ),
    ):
        if not isinstance(row.body, EntityBody) or any(
            value["id"] == "entity:" + row.reference.id for value in evidence_nodes
        ):
            continue
        evidence_nodes.append(
            json_object(
                {
                    "id": "entity:" + row.reference.id,
                    "type": "entity",
                    "content": row.body.label,
                    "metadata": {"hash": row.reference.id, "reference": row.reference},
                }
            )
        )
    for row in related:
        value = edge(row, known)
        evidence_nodes.append(
            json_object(
                {
                    "id": "relation:" + row.reference.id,
                    "type": "relation",
                    "content": value["text"],
                    "metadata": value,
                }
            )
        )
        assert isinstance(row.body, AssociationBody)
        evidence_edges.extend(
            [
                {
                    "source": "entity:" + row.body.from_ref.id,
                    "target": "relation:" + row.reference.id,
                    "kind": "subject",
                    "label": "subject",
                    "weight": 1.0,
                    "weight_basis": "structural_membership",
                },
                {
                    "source": "relation:" + row.reference.id,
                    "target": "entity:" + row.body.to_ref.id,
                    "kind": "object",
                    "label": "object",
                    "weight": 1.0,
                    "weight_basis": "structural_membership",
                },
            ]
        )
        for ref in paragraphs(row, snapshot):
            if ref.reference.exact() in supporting:
                evidence_edges.append(
                    {
                        "source": "paragraph:" + ref.reference.id,
                        "target": "relation:" + row.reference.id,
                        "kind": "supports",
                        "label": "supports",
                        "weight": 1.0,
                        "weight_basis": "structural_membership",
                    }
                )
    evidence_nodes.extend(
        json_object(
            {
                "id": "paragraph:" + row.reference.id,
                "type": "paragraph",
                "content": row.body.text,
                "metadata": paragraph(row, snapshot),
            }
        )
        for row in selected
        if isinstance(row.body, MemoryBody)
    )
    truncated = len(evidence_nodes) > evidence_node_limit
    evidence_nodes = evidence_nodes[:evidence_node_limit]
    allowed = {str(row["id"]) for row in evidence_nodes}
    graph = json_object(
        {
            "nodes": evidence_nodes,
            "edges": [
                row
                for row in evidence_edges
                if str(row["source"]) in allowed and str(row["target"]) in allowed
            ],
            "truncated": truncated,
            "focus_entities": [
                value.body.label
                for value in (left, right)
                if value and isinstance(value.body, EntityBody)
            ],
        }
    )
    result = json_object(
        {
            "success": True,
            "relations": relations,
            "paragraphs": [paragraph(row, snapshot) for row in selected],
            "evidence_graph": graph,
            "watermark": snapshot.watermark,
        }
    )
    if node:
        assert isinstance(left.body, EntityBody)
        result["node"] = json_object(
            {
                "id": left.body.label,
                "type": "entity",
                "content": left.body.label,
                "hash": left.reference.id,
                "reference": left.reference,
                "active_evidence_count": len(paragraphs(left, snapshot)),
                "source_roots": left.source_roots,
            }
        )
    else:
        assert (
            isinstance(left.body, EntityBody)
            and right is not None
            and isinstance(right.body, EntityBody)
        )
        weights = [
            row.body.weight
            for row in related
            if isinstance(row.body, AssociationBody) and row.body.weight is not None
        ]
        result["edge"] = json_object(
            {
                "source": left.body.label,
                "target": right.body.label,
                "weight": sum(weights) / len(weights) if weights else 1.0,
                "recorded_weight": sum(weights) / len(weights) if weights else None,
                "weight_basis": "recorded" if weights else "neutral_prior",
                "relation_hashes": [row.reference.id for row in related],
                "predicates": [
                    row.body.relation for row in related if isinstance(row.body, AssociationBody)
                ],
            }
        )
    return result
