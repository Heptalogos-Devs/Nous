"""Follow actual evidence through the shared source-read owners in one view."""

from __future__ import annotations

from collections import deque
from typing import TYPE_CHECKING

from nous_wave.contracts import (
    CallContext,
    DomainError,
    MediaReference,
    MemoryBody,
)
from nous_wave.contracts.context import CognitiveTarget, ContextTarget, MediaTarget
from nous_wave.contracts.source_graph import ExpandedSources, ExpandSources, SourceEdge, SourceNode
from pydantic import JsonValue, TypeAdapter

from nous.material.service import require_observation_in

from .assets import record_text

if TYPE_CHECKING:
    from .service import RetrievalService

_UNAVAILABLE = {
    "source_unavailable",
    "region_not_found",
    "representation_not_found",
    "derived_region_not_found",
    "cognition_not_found",
    "reference_not_found",
}


def target_key(target: ContextTarget) -> str:
    key = target.reference.exact()
    if isinstance(target, MediaTarget) and target.reference.observation_id:
        key += "#" + target.reference.observation_id
    return key


def prefix(text: str, remaining: int) -> str:
    return text.encode("utf-8")[:remaining].decode("utf-8", errors="ignore")


async def expand_sources(
    self: RetrievalService, ctx: CallContext, command: ExpandSources
) -> ExpandedSources:
    roots = tuple(target_key(target) for target in command.targets)
    queue: deque[tuple[ContextTarget | str, str | None, int, bool]] = deque(
        (target, None, 0, True) for target in command.targets
    )
    nodes: dict[str, SourceNode] = {}
    edges: list[SourceEdge] = []
    remaining = command.text_bytes
    complete = True
    async with self.authority.read(ctx) as view:

        def observation_key(identity: str, region: str | None) -> str:
            return "observation:" + identity + ("#" + region if region else "")

        def link(
            parent: str,
            child: ContextTarget | str,
            region: str | None,
            depth: int,
            polarity: str = "support",
            role: str = "grounding",
        ) -> None:
            nonlocal complete
            key = observation_key(child, region) if isinstance(child, str) else target_key(child)
            if depth > command.max_depth:
                complete = False
                return
            edges.append(SourceEdge(source=parent, target=key, polarity=polarity, role=role))
            queue.append((child, region, depth, False))

        while queue:
            target, region_id, depth, root = queue.popleft()
            key = (
                observation_key(target, region_id)
                if isinstance(target, str)
                else target_key(target)
            )
            if key in nodes:
                continue
            if len(nodes) >= command.max_nodes:
                complete = False
                break
            try:
                if isinstance(target, str):
                    row = await require_observation_in(view, ctx, target, region_id=region_id)
                    nodes[key] = SourceNode(
                        key=key,
                        observation_id=target,
                        material_id=str(row["material_id"]),
                        observation=TypeAdapter(dict[str, JsonValue]).validate_python(
                            row["record"]
                        ),
                    )
                    if region_id:
                        link(
                            key,
                            MediaTarget(
                                reference=MediaReference(
                                    kind="source_region", id=region_id, observation_id=target
                                )
                            ),
                            None,
                            depth + 1,
                        )
                elif isinstance(target, CognitiveTarget):
                    record = await self.cognition.get_in(view, ctx, target.reference)
                    text = prefix(record_text(record), remaining)
                    remaining -= len(text.encode("utf-8"))
                    complete &= len(text.encode("utf-8")) == len(
                        record_text(record).encode("utf-8")
                    )
                    nodes[key] = SourceNode(key=key, target=target, text=text)
                    for evidence in record.evidence:
                        child = (
                            CognitiveTarget(reference=evidence.reference)
                            if evidence.reference
                            else evidence.observation_id
                        )
                        if child is not None:
                            link(
                                key,
                                child,
                                evidence.region_id,
                                depth + 1,
                                evidence.polarity,
                                evidence.role,
                            )
                    direct = {e.observation_id for e in record.evidence if e.observation_id}
                    for oid in record.source_roots:
                        if oid not in direct:
                            for region in record.source_regions.get(oid, ()) or (None,):
                                link(key, oid, region, depth + 1, role="source_root")
                    if isinstance(record.body, MemoryBody):
                        for media in record.body.media:
                            link(
                                key,
                                MediaTarget(reference=media),
                                None,
                                depth + 1,
                                role="media_input",
                            )
                else:
                    reference = target.reference
                    info = await self.derivations.source_in(view, ctx, reference)
                    text = ""
                    if reference.kind in {"representation", "derived_region"}:
                        record = await self.derivations.media_record_in(view, ctx, reference)
                        text = prefix(record.text, remaining)
                        remaining -= len(text.encode("utf-8"))
                        complete &= len(text.encode("utf-8")) == len(record.text.encode("utf-8"))
                    nodes[key] = SourceNode(key=key, target=target, text=text)
                    for oid in info.roots:
                        for region in info.regions.get(oid, ()) or (None,):
                            link(key, oid, region, depth + 1, role="media_source")
            except DomainError as error:
                if root or error.code not in _UNAVAILABLE:
                    raise
                nodes[key] = SourceNode(key=key, status="unavailable", reason=error.code)
                complete = False
        return ExpandedSources(
            roots=roots,
            nodes=tuple(nodes.values()),
            edges=tuple(edge for edge in edges if edge.target in nodes),
            watermark=view.commit_seq,
            complete=complete,
        )
