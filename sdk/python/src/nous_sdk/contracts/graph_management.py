"""Source-backed manual graph operations over canonical cognitive revisions."""

from typing import Literal

from pydantic import Field, JsonValue

from .cognition import CognitiveRecord, CognitiveRef
from .common import Contract, Operation


class GraphChange(Operation):
    action: Literal[
        "create_node",
        "rename_node",
        "delete_node",
        "create_edge",
        "update_edge_weight",
        "delete_edge",
    ]
    reference: CognitiveRef | None = None
    name: str | None = None
    new_name: str | None = None
    subject: str | None = None
    predicate: str | None = None
    object: str | None = None
    relation_hash: str | None = None
    weight: float | None = Field(default=None, ge=0, le=1)
    confidence: float | None = Field(default=None, ge=0, le=1)
    source_paragraph: str | None = None
    metadata: dict[str, JsonValue] = Field(default_factory=dict)
    reported_by: str | None = None
    reason: str = Field(default="Manual graph management", min_length=1, max_length=4096)


class GraphSnapshot(Contract):
    records: tuple[CognitiveRecord, ...]
    paragraphs: tuple[CognitiveRecord, ...]
    watermark: int


class GraphChangeResult(Contract):
    operation_id: str
    records: tuple[CognitiveRecord, ...]
    created: tuple[CognitiveRef, ...] = ()
    revived: tuple[CognitiveRef, ...] = ()
    report_material_id: str
    observation_id: str
    commit_seq: int
    retired_vector_associations: int = Field(ge=0)
