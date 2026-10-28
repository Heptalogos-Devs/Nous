from __future__ import annotations

from dataclasses import dataclass

import numpy as np
from nous_wave.contracts import (
    CognitiveRecord,
    MediaRecord,
)
from numpy.typing import NDArray
from pydantic import BaseModel, TypeAdapter

from nous.cognition.projection import body_text
from nous.material.service import SourceRecord


class VectorAsset(BaseModel):
    reference: str
    space: str
    model: str
    dimensions: int
    vector: list[float]
    call_id: str | None
    commit_seq: int


VECTOR_ROWS = TypeAdapter(list[VectorAsset])

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
