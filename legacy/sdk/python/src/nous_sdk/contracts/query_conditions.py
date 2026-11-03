"""Machine query inputs share the natural-language retrieval owner."""

from __future__ import annotations

from typing import Annotated, Literal

from pydantic import Field, model_validator

from .common import Contract

QueryAuthority = Literal[
    "subject_cognition",
    "external_current_authority",
    "evidence",
    "interpretation",
    "resource_descriptor",
]
QueryModality = Literal["text", "image", "audio", "video", "document", "binary"]
QueryMode = Literal["lexical", "dense", "hybrid", "vcp"]
QueryEffectiveMode = QueryMode | Literal["mixed"]


class QueryAtom(Contract):
    operation: Literal["atom"] = "atom"
    source: str = Field(max_length=65536)


class QueryAny(Contract):
    operation: Literal["any"] = "any"
    children: tuple[QueryExpression, ...] = Field(min_length=1, max_length=64)


class QueryAll(Contract):
    operation: Literal["all"] = "all"
    children: tuple[QueryExpression, ...] = Field(min_length=1, max_length=64)


QueryExpression = Annotated[QueryAtom | QueryAny | QueryAll, Field(discriminator="operation")]
QueryAny.model_rebuild()
QueryAll.model_rebuild()


class OriginMatch(Contract):
    field: str = Field(min_length=1, max_length=128)
    value: str = Field(max_length=4096)


class QueryFilters(Contract):
    authority: QueryAuthority | None = None
    modalities: tuple[QueryModality, ...] = ()
    sources: tuple[str, ...] | None = None
    source_ids: tuple[str, ...] | None = None
    source_types: tuple[str, ...] | None = None
    origin: tuple[OriginMatch, ...] = ()
    tags: tuple[str, ...] = ()
    cognitive_roles: tuple[str, ...] = ()
    formation_modes: tuple[Literal["grounded", "synthesized"], ...] = ()
    epistemic_classes: tuple[
        Literal["observed", "reported", "derived", "inferred", "narrative", "simulated"], ...
    ] = ()


class QueryBudget(Contract):
    vcp_concepts: int = Field(default=32, ge=0, le=256)
    vcp_text_bytes: int = Field(default=65536, ge=0, le=1048576)
    candidates: int = Field(default=256, ge=1, le=10000)
    branches: int = Field(default=32, ge=1, le=64)
    depth: int = Field(default=8, ge=1, le=16)
    source_depth: int = Field(default=4, ge=0, le=16)
    source_nodes: int = Field(default=128, ge=1, le=1024)
    source_text_bytes: int = Field(default=65536, ge=0, le=1048576)
    rerank_documents: int = Field(default=32, ge=1, le=256)
    rerank_document_bytes: int = Field(default=4096, ge=1, le=65536)
    rerank_total_bytes: int = Field(default=65536, ge=1, le=1048576)
    diagnostics_bytes: int = Field(default=65536, ge=0, le=1048576)


class QueryCapabilities(Contract):
    semantic: Literal["required", "optional"] = "required"
    exploration: Literal["required", "optional"] = "required"
    require_complete_vectors: bool = False


class QueryExecution(Contract):
    requested_mode: QueryMode
    effective_mode: QueryEffectiveMode
    candidate_budget: int
    candidates_returned: int
    eligible_objects: int = 0
    missing_vectors: int = 0
    candidate_limited: bool = False
    degraded: bool = False
    reasons: tuple[str, ...] = ()

    @model_validator(mode="after")
    def bounded_candidates(self) -> QueryExecution:
        if self.candidates_returned > self.candidate_budget:
            raise ValueError("Query candidates exceeded the admitted budget")
        return self
