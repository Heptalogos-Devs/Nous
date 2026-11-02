"""Final public result expression, authorized provenance and optional model order."""

from __future__ import annotations

import json
from hashlib import sha256
from typing import TYPE_CHECKING
from uuid import uuid4

from nous_sdk.contracts import (
    CallContext,
    CognitiveRecord,
    DomainError,
    MediaRecord,
    QueryHit,
    QueryRequest,
    QueryResult,
)
from nous_sdk.contracts.context import CognitiveTarget, ContextTarget, MediaTarget
from nous_sdk.contracts.query_readout import RerankInfo
from nous_sdk.contracts.resources import ResourceRecord
from nous_sdk.contracts.source_graph import ExpandSources
from nous_sdk.contracts.source_objects import EvidenceRelationRecord, ObservationRecord
from pydantic import BaseModel, Field

from nous.platform.persistence import ReadView

from .assets import RetrievalRecord, record_text
from .cross_encoder import score_pairs
from .filters import authority
from .media import source_material_ids_in
from .profiles import WavePolicy
from .source_expansion import prefix

if TYPE_CHECKING:
    from .service import RetrievalService


class DocumentScore(BaseModel):
    key: str
    score: float = Field(allow_inf_nan=False)


class Ranking(BaseModel):
    scores: tuple[DocumentScore, ...] = Field(min_length=1)


async def require_record_in(
    self: RetrievalService,
    view: ReadView,
    ctx: CallContext,
    record: RetrievalRecord,
    *,
    current_selection: bool = False,
) -> None:
    if isinstance(record, CognitiveRecord):
        if current_selection:
            if await self.cognition.automatic_in(view, ctx, record.reference) is None:
                raise DomainError(
                    "cognition_inactive", "The selected cognition is no longer active"
                )
        else:
            await self.cognition.get_in(view, ctx, record.reference)
    elif isinstance(record, MediaRecord):
        await source_material_ids_in(view, ctx, record.reference, self.materials, self.derivations)
    elif isinstance(record, (ObservationRecord, EvidenceRelationRecord)):
        await self.references.require_source_object_in(view, ctx, record)
        if current_selection and isinstance(record, EvidenceRelationRecord):
            if await self.cognition.automatic_in(view, ctx, record.target) is None:
                raise DomainError("evidence_not_found", "The evidence target is no longer active")
    else:
        if self.resources is None:
            raise DomainError(
                "resource_directory_unavailable", "External resource directory is unavailable"
            )
        checked = await self.resources.get(ctx, record.reference.id, record.reference.revision)
        if record.snapshot and checked.snapshot is None:
            raise DomainError("source_unavailable", "Resource snapshot sources are unavailable")


def source_targets(record: RetrievalRecord) -> tuple[ContextTarget, ...]:
    if isinstance(record, CognitiveRecord):
        return (CognitiveTarget(reference=record.reference),)
    if isinstance(record, MediaRecord):
        return (MediaTarget(reference=record.reference),)
    if isinstance(record, ResourceRecord):
        return (MediaTarget(reference=record.snapshot.source),) if record.snapshot else ()
    if isinstance(record, ObservationRecord):
        return (MediaTarget(reference=record.source),)
    return (
        CognitiveTarget(reference=record.target),
        *((MediaTarget(reference=record.source),) if record.source else ()),
    )


async def visible_hits(
    self: RetrievalService,
    ctx: CallContext,
    ranked: list[tuple[RetrievalRecord, float]],
    limit: int,
    *,
    current_selection: bool = False,
) -> list[QueryHit]:
    hits: list[QueryHit] = []
    async with self.authority.read(ctx) as current:
        for record, score in ranked:
            try:
                await require_record_in(
                    self, current, ctx, record, current_selection=current_selection
                )
                visible = True
            except DomainError as error:
                if error.code not in {
                    "source_unavailable",
                    "region_not_found",
                    "representation_not_found",
                    "derived_region_not_found",
                    "cognition_inactive",
                    "cognition_not_found",
                    "evidence_not_found",
                    "resource_disabled",
                    "resource_not_found",
                }:
                    raise
                visible = False
            if visible:
                hits.append(
                    QueryHit(
                        object=record,
                        score=score,
                        source_roots=record.source_roots,
                        authority=authority(record),
                    )
                )
            if len(hits) >= limit:
                break
    return hits


async def _revalidate(
    self: RetrievalService, ctx: CallContext, result: QueryResult, *, current_selection: bool
) -> None:
    async with self.authority.read(ctx) as view:
        for hit in result.hits:
            await require_record_in(
                self, view, ctx, hit.object, current_selection=current_selection
            )


async def finalize(
    self: RetrievalService,
    ctx: CallContext,
    request: QueryRequest,
    result: QueryResult,
    *,
    explicit_sort: bool = False,
    current_selection: bool = False,
) -> QueryResult:
    algorithm = request.reranker or ("llm" if request.rerank != "off" else "compute")
    strict = request.capability_policy == "strict" or request.rerank == "required"
    before = tuple(hit.object.reference.exact() for hit in result.hits)
    if algorithm == "llm":
        await _revalidate(self, ctx, result, current_selection=current_selection)
    info = RerankInfo(
        mode=request.rerank,
        status="skipped",
        retained_base_order=len(result.hits),
        reason="disabled",
    )
    hits = result.hits
    if algorithm == "llm":
        if explicit_sort:
            if strict:
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
                if strict:
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
                    generated = await self.models.generate(
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
                    info = RerankInfo(
                        mode=request.rerank,
                        status="completed",
                        call_id=call_id,
                        invocation_id=invocation,
                        scored_documents=len(scores),
                        retained_base_order=len(hits) - len(scores),
                    )
                except DomainError as error:
                    if strict or error.code not in {
                        "model_unavailable",
                        "model_not_configured",
                        "capability_unavailable",
                        "configuration_missing",
                        "model_contract_unavailable",
                    }:
                        raise
                    info = RerankInfo(
                        mode=request.rerank,
                        status="skipped",
                        invocation_id=invocation,
                        retained_base_order=len(hits),
                        reason=str(error),
                    )
    if algorithm == "compute":
        info = info.model_copy(
            update={"status": "completed", "reason": None, "scored_documents": len(hits)}
        )
    elif algorithm == "cross_encoder" and hits:
        remaining = request.budget.rerank_total_bytes
        cross_documents: list[str] = []
        indices: list[int] = []
        for index, hit in enumerate(hits[: request.budget.rerank_documents]):
            text = prefix(
                record_text(hit.object), min(request.budget.rerank_document_bytes, remaining)
            )
            if text.strip():
                cross_documents.append(text)
                indices.append(index)
                remaining -= len(text.encode("utf-8"))
            if remaining == 0:
                break
        try:
            effective = await self.configuration.effective(ctx, "wave")
            policy = WavePolicy.model_validate(effective["value"])
            profile = policy.profiles[request.wave_profile or policy.default_profile]
            values = await score_pairs(profile.cross_encoder_model, request.text, cross_documents)
            scores = dict(zip(indices, values, strict=True))
            order = sorted(scores, key=lambda index: (-scores[index], index))
            hits = tuple(
                hits[index].model_copy(update={"rerank_score": scores[index]}) for index in order
            ) + tuple(hit for index, hit in enumerate(hits) if index not in scores)
            info = info.model_copy(
                update={
                    "status": "completed",
                    "reason": None,
                    "scored_documents": len(scores),
                    "retained_base_order": len(hits) - len(scores),
                }
            )
        except DomainError as error:
            if strict or error.code != "reranker_unavailable":
                raise
            info = info.model_copy(update={"reason": error.code})
    info = info.model_copy(
        update={
            "algorithm": algorithm,
            "before": before,
            "after": tuple(hit.object.reference.exact() for hit in hits),
        }
    )
    await _revalidate(self, ctx, result, current_selection=current_selection)
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
                targets=tuple(target for hit in hits for target in source_targets(hit.object)),
                max_depth=request.budget.source_depth,
                max_nodes=request.budget.source_nodes,
                text_bytes=request.budget.source_text_bytes,
            ),
        )
    return result.model_copy(
        update={"hits": hits, "expression": "\n\n".join(lines), "sources": sources, "rerank": info}
    )
