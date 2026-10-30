"""Expose grounded method routes with authorized canonical references."""

from __future__ import annotations

from typing import TYPE_CHECKING, Literal

from nous_sdk.contracts import (
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    DomainError,
    MediaReference,
    QueryRequest,
)
from nous_sdk.contracts.common import Contract
from nous_sdk.contracts.query_vcp import (
    QueryThinking,
    ThinkingRoute,
    ThinkingSelection,
    ThinkingStage,
)

from .assets import RetrievalRecord
from .media import source_material_ids_in

if TYPE_CHECKING:
    from .service import RetrievalService


class Route(Contract):
    source: str
    target: str
    previous: str | None
    hop: int
    flow: float
    wormhole: bool
    immediate_return: bool


class Selection(Contract):
    reference: str
    score: float
    parents: tuple[str, ...]
    relation: Literal["root", "continuation", "analogy", "supplement"]
    route: Route | None


class Stage(Contract):
    name: str
    limit: int
    selections: tuple[Selection, ...]


class Thinking(Contract):
    status: Literal["ready", "unavailable", "not_requested"]
    reason: str | None = None
    generation: str | None = None
    artifact_signature: str | None = None
    stages: tuple[Stage, ...] = ()


async def resolve_thinking(
    self: RetrievalService,
    ctx: CallContext,
    value: object,
    catalog: list[RetrievalRecord],
    records: list[RetrievalRecord],
    request: QueryRequest,
    reasons: list[str],
) -> QueryThinking:
    def unavailable(reason: str) -> QueryThinking:
        if request.capabilities.exploration == "required":
            raise DomainError(
                "thinking_unavailable",
                "The accepted observation has no available grounded method plan",
                reason=reason,
                capability="vcp_thinking",
            )
        reasons.append("thinking_unavailable")
        return QueryThinking(status="unavailable", reason=reason)

    result = Thinking.model_validate(value)
    if result.status != "ready":
        return unavailable(result.reason or "thinking_not_executed")
    candidates = {record.reference.exact(): record for record in records}
    graph = {
        record.reference.exact(): record
        for record in catalog
        if isinstance(record, CognitiveRecord)
    }
    async with self.authority.read(ctx) as view:
        checked: set[str] = set()

        async def candidate(key: str) -> CognitiveRef | MediaReference:
            record = candidates.get(key)
            if record is None:
                raise DomainError(
                    "native_thinking_contract",
                    "A method selection escaped the authorized candidate snapshot",
                )
            if key not in checked:
                if isinstance(record, CognitiveRecord):
                    await self.cognition.get_in(view, ctx, record.reference)
                else:
                    await source_material_ids_in(
                        view, ctx, record.reference, self.materials, self.derivations
                    )
                checked.add(key)
            return record.reference

        async def node(key: str) -> CognitiveRef:
            record = graph.get(key)
            if record is None:
                raise DomainError(
                    "native_thinking_contract", "A route escaped the authorized graph snapshot"
                )
            if key not in checked:
                await self.cognition.get_in(view, ctx, record.reference)
                checked.add(key)
            return record.reference

        stages: list[ThinkingStage] = []
        try:
            for stage in result.stages:
                selections: list[ThinkingSelection] = []
                for item in stage.selections:
                    route = item.route
                    selections.append(
                        ThinkingSelection(
                            reference=await candidate(item.reference),
                            score=item.score,
                            parents=tuple([await candidate(key) for key in item.parents]),
                            relation=item.relation,
                            route=ThinkingRoute(
                                source=await node(route.source),
                                target=await node(route.target),
                                previous=await node(route.previous) if route.previous else None,
                                hop=route.hop,
                                flow=route.flow,
                                wormhole=route.wormhole,
                                immediate_return=route.immediate_return,
                            )
                            if route
                            else None,
                        )
                    )
                stages.append(
                    ThinkingStage(name=stage.name, limit=stage.limit, selections=tuple(selections))
                )
        except DomainError as error:
            if error.code not in {
                "source_unavailable",
                "cognition_not_found",
                "source_purged",
                "region_not_found",
                "representation_not_found",
                "derived_region_not_found",
            }:
                raise
            return unavailable(error.code)
    return QueryThinking(
        status="ready",
        generation=result.generation,
        artifact_signature=result.artifact_signature,
        stages=tuple(stages),
    )
