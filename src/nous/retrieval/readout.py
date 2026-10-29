"""Final public result expression, authorized provenance and optional model order."""

from __future__ import annotations

import json
from hashlib import sha256
from typing import TYPE_CHECKING
from uuid import uuid4

from nous_wave.contracts import CallContext, CognitiveRecord, DomainError, QueryRequest, QueryResult
from nous_wave.contracts.context import CognitiveTarget, MediaTarget
from nous_wave.contracts.query_readout import RerankInfo
from nous_wave.contracts.source_graph import ExpandSources
from pydantic import BaseModel, Field

from .assets import record_text
from .source_expansion import prefix

if TYPE_CHECKING:
    from .service import RetrievalService


class DocumentScore(BaseModel):
    key: str
    score: float = Field(allow_inf_nan=False)


class Ranking(BaseModel):
    scores: tuple[DocumentScore, ...] = Field(min_length=1)


async def _revalidate(self: RetrievalService, ctx: CallContext, result: QueryResult) -> None:
    async with self.authority.read(ctx) as view:
        for hit in result.hits:
            if isinstance(hit.object, CognitiveRecord):
                await self.cognition.get_in(view, ctx, hit.object.reference)
            else:
                await self.derivations.source_in(view, ctx, hit.object.reference)


async def finalize(
    self: RetrievalService,
    ctx: CallContext,
    request: QueryRequest,
    result: QueryResult,
    *,
    explicit_sort: bool = False,
) -> QueryResult:
    await _revalidate(self, ctx, result)
    info = RerankInfo(
        mode=request.rerank,
        status="skipped",
        retained_base_order=len(result.hits),
        reason="disabled",
    )
    hits = result.hits
    if request.rerank != "off":
        if explicit_sort:
            if request.rerank == "required":
                raise DomainError(
                    "rerank_sort_conflict",
                    "Explicit result sorting conflicts with required model reranking",
                )
            info = info.model_copy(update={"reason": "explicit_sort"})
        elif not hits:
            info = info.model_copy(update={"reason": "no_candidates"})
        else:
            remaining = request.budget.rerank_total_bytes
            documents: list[dict[str, str]] = []
            for index, hit in enumerate(hits[: request.budget.rerank_documents]):
                text = prefix(
                    record_text(hit.object), min(request.budget.rerank_document_bytes, remaining)
                )
                if not text.strip():
                    continue
                documents.append({"key": str(index), "text": text})
                remaining -= len(text.encode("utf-8"))
                if remaining == 0:
                    break
            if not documents:
                if request.rerank == "required":
                    raise DomainError(
                        "rerank_text_required",
                        "Required text-generation reranking has no readable text candidates",
                    )
                info = info.model_copy(update={"reason": "no_text_candidates"})
            else:
                snapshot = {
                    "query": request.model_dump(mode="json"),
                    "documents": documents,
                    "references": [hit.object.reference.exact() for hit in hits],
                }
                digest = sha256(
                    json.dumps(snapshot, sort_keys=True, ensure_ascii=False).encode()
                ).hexdigest()
                invocation = (
                    "query-rerank:"
                    + sha256((request.rerank_invocation_id or str(uuid4())).encode()).hexdigest()
                    + ":"
                    + digest
                )
                call_id: str | None = None
                try:
                    generated = await self.models.structured(
                        ctx,
                        "query_rerank",
                        {"query": request.text, "documents": documents},
                        Ranking,
                        source_ids=tuple(
                            sorted({root for hit in hits for root in hit.source_roots})
                        ),
                        invocation_id=invocation,
                    )
                    call_id = generated.call_id
                    scored = generated.output.scores
                    available = {document["key"] for document in documents}
                    keys = [item.key for item in scored]
                    if len(keys) != len(set(keys)) or not set(keys).issubset(available):
                        raise DomainError(
                            "rerank_output_invalid",
                            "Reranking returned duplicate or unknown candidate keys",
                        )
                    scores = {int(item.key): item.score for item in scored}
                    order = sorted(scores, key=lambda index: (-scores[index], index))
                    hits = tuple(
                        hits[index].model_copy(update={"rerank_score": scores[index]})
                        for index in order
                    ) + tuple(hit for index, hit in enumerate(hits) if index not in scores)
                    await _revalidate(self, ctx, result)
                    info = RerankInfo(
                        mode=request.rerank,
                        status="completed",
                        call_id=call_id,
                        invocation_id=invocation,
                        scored_documents=len(scores),
                        retained_base_order=len(hits) - len(scores),
                    )
                except DomainError as error:
                    if request.rerank != "optional" or error.code != "model_unavailable":
                        raise
                    info = RerankInfo(
                        mode=request.rerank,
                        status="skipped",
                        invocation_id=invocation,
                        retained_base_order=len(hits),
                        reason=str(error),
                    )
    await _revalidate(self, ctx, result)
    remaining = request.budget.source_text_bytes
    lines: list[str] = []
    for hit in hits:
        text = prefix(record_text(hit.object), remaining)
        remaining -= len(text.encode("utf-8"))
        address = (
            hit.object.addresses.exact if hit.object.addresses else hit.object.reference.exact()
        )
        lines.append("[" + address + "] " + text)
    sources = None
    if request.materialize and hits:
        sources = await self.expand_sources(
            ctx,
            ExpandSources(
                targets=tuple(
                    CognitiveTarget(reference=hit.object.reference)
                    if isinstance(hit.object, CognitiveRecord)
                    else MediaTarget(reference=hit.object.reference)
                    for hit in hits
                ),
                max_depth=request.budget.source_depth,
                max_nodes=request.budget.source_nodes,
                text_bytes=request.budget.source_text_bytes,
            ),
        )
    return result.model_copy(
        update={"hits": hits, "expression": "\n\n".join(lines), "sources": sources, "rerank": info}
    )
