"""Typed immutable snapshot mapping shared by construction and queries."""

from pydantic import JsonValue
from typing_extensions import TypedDict


class ConceptMapping(TypedDict):
    key: str
    label: str
    description: str
    aliases: list[str]


class DocumentMapping(TypedDict):
    artifact: dict[str, str]
    title: str | None
    selector: dict[str, JsonValue]
    roots: list[str]
    derived_from: list[str]
    times: dict[str, object]
    declared_times: dict[str, object] | None
    artifact_times: dict[str, str | None]


class CandidateMapping(TypedDict):
    key: str
    content: dict[str, object]
    origin_contents: list[dict[str, object]]
    retrieval_content: dict[str, object]
    content_document: str
    document: str
    group_key: str
    roots: list[str]
    concepts: list[str]
    paths: list[dict[str, object]]
    metadata: dict[str, JsonValue]
    annotation: dict[str, str] | None


class AssociationMapping(TypedDict):
    key: str
    source: str
    target: str
    relation: str
    directed: bool
    strength: float
    confidence: float
    polarity: str
    support_group: str
    roots: list[str]


class SnapshotMapping(TypedDict):
    owner: dict[str, JsonValue]
    name: str
    metadata: dict[str, JsonValue]
    root_order: list[str]
    documents: dict[str, DocumentMapping]
    roots: dict[str, list[dict[str, str]]]
    concepts: list[ConceptMapping]
    candidates: list[CandidateMapping]
    associations: list[AssociationMapping]
    support_groups: list[dict[str, object]]


class SnapshotRow(TypedDict):
    id: str
    identifier: str
    collection_id: str
    manifest_artifact_id: str | None
    covered_generation: int | None
    profile: str | None
    space_id: str | None
    space_definition: dict[str, object] | None
    identity: dict[str, JsonValue]
    index_location: str
    mapping: SnapshotMapping
    owner: dict[str, JsonValue]
    metadata: dict[str, JsonValue]
    build_key: str | None
    build_signature: str | None
    component_signature: str
    graph_signature: str
    build_summary: dict[str, JsonValue]
    created_at: str
