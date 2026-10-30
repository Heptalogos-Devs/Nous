"""Resolve request-owned cores and transient concepts against one authorized snapshot."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

import numpy as np
from nous_wave.contracts import (
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    DomainError,
    EmbeddingBatch,
    QueryRequest,
)
from nous_wave.contracts.query_vcp import QueryConcept, QueryConceptResult
from numpy.typing import NDArray
from pydantic import JsonValue

from nous.material.service import SourceRecord

from .assets import GraphAssets, RetrievalRecord, record_text
from .binding import BoundSelection
from .planning import BoundPlan
from .policy import ResolvedPolicy
from .work import QueryWork

if TYPE_CHECKING:
    from .service import RetrievalService


@dataclass(frozen=True)
class BoundVcp:
    core: tuple[CognitiveRef, ...]
    concepts: tuple[QueryConcept, ...]
    source_ids: tuple[str, ...]
    thinking: bool
    terms: tuple[str, ...]

    def options(
        self, policy: ResolvedPolicy, limit: int, diagnostics: str, core: tuple[CognitiveRef, ...]
    ) -> dict[str, JsonValue]:
        result = policy.options(limit, diagnostics)
        vcp = result["vcp"]
        if not isinstance(vcp, dict):
            raise ValueError("VCP policy must have its typed configuration object")
        vcp["core_keys"] = [ref.exact() for ref in core]
        vcp["ghost_tags"] = [{"name": cue.text, "is_core": cue.is_core} for cue in self.concepts]
        vcp["thinking"] = self.thinking
        return result

    def graph_core(
        self, graph: GraphAssets, request: QueryRequest, reasons: list[str]
    ) -> tuple[CognitiveRef, ...]:
        available = {tag["key"] for tag in graph.tags}
        missing = tuple(ref for ref in self.core if ref.exact() not in available)
        if missing:
            if request.capabilities.exploration == "required":
                raise DomainError(
                    "vcp_core_unavailable",
                    "Exact cores lack authorized active vectors in the query space",
                    references=[ref.model_dump(mode="json") for ref in missing],
                )
            reasons.append("vcp_core_vectors_unavailable")
        return tuple(ref for ref in self.core if ref.exact() in available)


def resolve_intent(
    request: QueryRequest,
    plan: BoundPlan,
    selection: BoundSelection,
    catalog: list[RetrievalRecord],
    sources: dict[str, SourceRecord],
    policy: ResolvedPolicy,
    work: QueryWork,
) -> BoundVcp:
    supplied = request.vcp
    if request.mode != "vcp" and (supplied.core or supplied.concepts or supplied.thinking):
        raise DomainError("invalid_query", "VCP request intent requires VCP mode")
    by_ref = {
        record.reference.exact(): record
        for record in catalog
        if isinstance(record, CognitiveRecord)
    }
    core: dict[str, CognitiveRef] = {ref.exact(): ref for ref in selection.core_references}
    roots: set[str] = set()
    terms: list[str] = []
    for ref in supplied.core:
        record = by_ref.get(ref.exact())
        if record is None:
            raise DomainError(
                "vcp_core_unavailable",
                "An exact core is unavailable in the authorized query snapshot",
            )
        core[ref.exact()] = ref
        roots.update(record.source_roots)
        terms.append(record_text(record))
    if request.reference in by_ref:
        record = by_ref[request.reference]
        core[record.reference.exact()] = record.reference
        roots.update(record.source_roots)
        terms.append(record_text(record))
    if not roots.issubset(sources):
        raise DomainError("source_unavailable", "A requested core has an unavailable original")
    concepts: dict[str, QueryConcept] = {}
    if request.mode == "vcp":
        for value in (
            *supplied.concepts,
            *(
                QueryConcept(text=cue.get("text", ""))
                for cue in plan["selectors"]
                if cue["kind"] == "concept" and cue.get("text")
            ),
        ):
            if not value.text.strip():
                raise DomainError("invalid_query", "Query concepts require nonempty text")
            previous = concepts.get(value.text)
            concepts[value.text] = value.model_copy(
                update={"is_core": value.is_core or bool(previous and previous.is_core)}
            )
    if core and not (
        request.text.strip() or request.parts or request.media or selection.terms or terms
    ):
        terms.extend(record_text(by_ref[key]) for key in core if key in by_ref)
    if request.mode == "vcp" and (
        len(core) + len(concepts) > request.budget.vcp_concepts
        or sum(len(value.text.encode("utf-8")) for value in concepts.values())
        + sum(len(text.encode("utf-8")) for text in terms)
        > request.budget.vcp_text_bytes
    ):
        raise DomainError("query_budget_exceeded", "VCP intent exceeds its concept or text budget")
    if request.mode == "vcp":
        work.cues(
            len(core) + len(concepts),
            sum(len(value.text.encode("utf-8")) for value in concepts.values())
            + sum(len(text.encode("utf-8")) for text in terms),
        )
    if plan.get("explore") and supplied.thinking is False:
        raise DomainError("invalid_query", "$explore conflicts with disabled request thinking")
    thinking = request.mode == "vcp" and (
        supplied.thinking
        if supplied.thinking is not None
        else bool(plan.get("explore") or policy.value.vcp.thinking)
    )
    if request.mode == "vcp" and not (
        request.text.strip()
        or request.parts
        or request.media
        or selection.terms
        or terms
        or concepts
    ):
        raise DomainError("invalid_query", "VCP requires query text, a core or a media input")
    return BoundVcp(
        tuple(core.values()),
        tuple(concepts.values()),
        tuple(sorted({sources[root]["material_id"] for root in roots})),
        thinking,
        tuple(terms),
    )


async def embed_concepts(
    self: RetrievalService, ctx: CallContext, intent: BoundVcp, query: EmbeddingBatch
) -> tuple[NDArray[np.float32] | None, tuple[QueryConceptResult, ...]]:
    if not intent.concepts:
        return None, ()
    # These are caller-supplied query cues, not readings or persisted evidence.
    batch = await self.models.embed(ctx, tuple(cue.text for cue in intent.concepts), source_ids=())
    if (batch.space, batch.model, batch.dimensions) != (query.space, query.model, query.dimensions):
        raise DomainError(
            "embedding_space_conflict",
            "Query concepts and query vector must use the same actual model space",
        )
    if (
        len(batch.vectors) != len(intent.concepts)
        or len(batch.call_ids) != len(intent.concepts)
        or any(call is None for call in batch.call_ids)
    ):
        raise DomainError(
            "invalid_embedding", "Query concepts require one vector and actual producer per cue"
        )
    vectors = np.ascontiguousarray(batch.vectors, dtype=np.float32)
    if vectors.shape != (len(intent.concepts), query.dimensions) or not np.isfinite(vectors).all():
        raise DomainError(
            "invalid_embedding", "Query concept vectors have invalid dimensions or coordinates"
        )
    results = tuple(
        QueryConceptResult(concept=cue, call_id=call, space=batch.space)
        for cue, call in zip(intent.concepts, batch.call_ids, strict=True)
        if call is not None
    )
    return vectors, results
