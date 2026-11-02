"""Namespace index publication. Only background writers construct native assets."""

from __future__ import annotations

import asyncio
import json
import math
import os
from dataclasses import dataclass
from hashlib import sha256
from typing import TYPE_CHECKING, NotRequired, TypedDict

import numpy as np
from nous_native import CancellationToken, IndexStore, Snapshot
from nous_sdk.contracts import QueryRequest
from nous.platform.call_context import CallContext
from numpy.typing import NDArray
from pydantic import BaseModel

from nous.platform.concurrency import owned_thread

from .accretion import consume
from .assets import GraphAssets, RetrievalRecord, VectorAsset, record_text
from .native import run_native
from .planning import bind
from .profiles import WavePolicy

if TYPE_CHECKING:
    from .service import RetrievalService


class Pointer(BaseModel):
    generation: str
    watermark: int
    graph_keys: tuple[str, ...] = ()
    fingerprint: str = ""
    space: str | None = None


class Manifest(BaseModel):
    usage: dict[str, float] = {}
    usage_through: int = 0
    usage_day: int = 0
    entries: dict[str, Pointer] = {}


class BuildOptions(TypedDict):
    tag_vectors: NotRequired[NDArray[np.float32]]
    tag_indptr: NotRequired[NDArray[np.uint64]]
    tag_indices: NotRequired[NDArray[np.uint64]]
    graph_indptr: NotRequired[NDArray[np.uint64]]
    graph_indices: NotRequired[NDArray[np.uint64]]
    graph_weights: NotRequired[NDArray[np.float64]]
    graph_polarities: NotRequired[NDArray[np.int8]]
    source_references: NotRequired[list[str]]
    candidate_source_indptr: NotRequired[NDArray[np.uint64]]
    candidate_source_indices: NotRequired[NDArray[np.uint64]]
    graph_source_indptr: NotRequired[NDArray[np.uint64]]
    graph_source_indices: NotRequired[NDArray[np.uint64]]


@dataclass(frozen=True)
class Published:
    snapshot: Snapshot
    watermark: int
    graph_keys: tuple[str, ...] = ()


def scope_key(namespace: str, space: str | None) -> str:
    return sha256(json.dumps([namespace, space]).encode()).hexdigest()


async def read_manifest(self: RetrievalService, ctx: CallContext) -> Manifest:
    pointer = self.index_path / (scope_key(ctx.namespace_id, None) + ".json")
    if not pointer.exists():
        return Manifest()
    return Manifest.model_validate_json(
        await owned_thread(lambda: pointer.read_text(encoding="utf-8"))
    )


async def publish_manifest(self: RetrievalService, ctx: CallContext, manifest: Manifest) -> None:
    pointer = self.index_path / (scope_key(ctx.namespace_id, None) + ".json")
    data = manifest.model_dump_json()

    def publish() -> None:
        temporary = pointer.with_suffix(".pending")
        with temporary.open("w", encoding="utf-8") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, pointer)

    await owned_thread(publish)


async def published_all(self: RetrievalService, ctx: CallContext) -> dict[str | None, Published]:
    manifest = await read_manifest(self, ctx)
    result: dict[str | None, Published] = {}
    for key, value in manifest.entries.items():
        cached = self.generations.get(key)
        if cached is not None and cached.snapshot.generation == value.generation:
            result[value.space] = Published(cached.snapshot, value.watermark, value.graph_keys)
            continue
        store = self.stores.get(key)
        if store is None:
            store = await run_native("open", None, IndexStore, str(self.index_path / key))
            self.stores[key] = store
        snapshot = await run_native("open_snapshot", None, store.open_snapshot, value.generation)
        asset = Published(snapshot, value.watermark, value.graph_keys)
        self.generations[key] = asset
        self.snapshots[key] = snapshot
        result[value.space] = asset
    return result


async def rebuild(self: RetrievalService, ctx: CallContext) -> dict[str, object]:
    """Publish a consistent source watermark, preserving other semantic spaces."""
    lock = self.build_locks.setdefault(ctx.namespace_id, asyncio.Lock())
    async with lock:
        request = QueryRequest(
            history=True,
            include_inactive=True,
            domains=("cognition", "media", "observation", "evidence", "resource")
            if self.resources is not None
            else ("cognition", "media", "observation", "evidence"),
        )
        _, plan = bind(ctx, request)
        view = await self.eligible(ctx, request, plan)
        records = sorted(view.records, key=lambda record: record.reference.exact())
        references = {record.reference.exact() for record in records}
        groups: dict[tuple[str, str, int], dict[str, VectorAsset]] = {}
        for asset in view.vectors:
            if (
                asset.reference in references
                and len(asset.vector) == asset.dimensions
                and np.isfinite(asset.vector).all()
                and any(asset.vector)
            ):
                groups.setdefault((asset.space, asset.model, asset.dimensions), {})[
                    asset.reference
                ] = asset
        manifest = await read_manifest(self, ctx)
        wave = WavePolicy.model_validate((await self.configuration.effective(ctx, "wave"))["value"])
        await consume(self, ctx, manifest, view.commit_seq, wave.accretion.usage_half_life_days)
        selections: list[
            tuple[
                str | None, list[RetrievalRecord], dict[str, VectorAsset] | None, GraphAssets | None
            ]
        ] = [(None, records, None, None)]
        for (space, _model, _dimensions), assets in groups.items():
            selected = [record for record in records if record.reference.exact() in assets]
            selections.append((space, selected, assets, None))
        built: list[str] = []
        for space, selected, assets, graph in selections:
            key = scope_key(ctx.namespace_id, space)
            current = manifest.entries.get(key)
            fingerprint = sha256(
                json.dumps(
                    [
                        "membership-cooccurrence-use-v1",
                        [(record.reference.exact(), record.commit_seq) for record in selected],
                        [(ref, asset.commit_seq) for ref, asset in sorted((assets or {}).items())],
                        [(record.reference.exact(), record.commit_seq) for record in view.catalog]
                        if assets
                        else [],
                        manifest.usage if assets else {},
                    ]
                ).encode()
            ).hexdigest()
            if current is not None and current.fingerprint == fingerprint:
                manifest.entries[key] = current.model_copy(update={"watermark": view.commit_seq})
                await publish_manifest(self, ctx, manifest)
                continue
            if assets:
                dimensions = next(iter(assets.values())).dimensions
                projection = self.graph(
                    view.catalog, selected, assets, dimensions, view.active_roots
                )
                graph = projection if projection.tags else None
            usage_gain: dict[str, float] = {}
            if graph is not None:
                for candidate, left, right in zip(
                    selected, graph.member_indptr[:-1], graph.member_indptr[1:], strict=True
                ):
                    count = manifest.usage.get(candidate.reference.id, 0.0)
                    if count:
                        for node in graph.member_indices[int(left) : int(right)]:
                            key_for_node = graph.tags[int(node)]["key"]
                            usage_gain[key_for_node] = usage_gain.get(
                                key_for_node, 0.0
                            ) + count / max(1, int(right - left))
                usage_gain = {
                    key: min(wave.accretion.maximum_usage_gain, math.log1p(value))
                    for key, value in usage_gain.items()
                }
            store = self.stores.get(key)
            if store is None:
                store = await run_native("open", None, IndexStore, str(self.index_path / key))
                self.stores[key] = store
            token = CancellationToken()
            options = graph_options(graph)
            snapshot = await run_native(
                "build",
                token,
                store.build,
                references=[record.reference.exact() for record in selected],
                texts=[record_text(record) for record in selected],
                base_generation=current.generation
                if current is not None and space is None
                else None,
                assets_json=json.dumps(
                    {
                        "space": space,
                        "tags": graph.tags,
                        "artifact_config": {
                            "usage_gain": usage_gain,
                            "usage_through": manifest.usage_through,
                        },
                    }
                    if graph
                    else {"space": space}
                    if space
                    else {}
                ),
                vectors=np.ascontiguousarray(
                    [assets[record.reference.exact()].vector for record in selected],
                    dtype=np.float32,
                )
                if assets
                else None,
                cancel=token,
                **options,
            )
            graph_keys = tuple(tag["key"] for tag in graph.tags) if graph else ()
            manifest.entries[key] = Pointer(
                generation=snapshot.generation,
                watermark=view.commit_seq,
                graph_keys=graph_keys,
                fingerprint=fingerprint,
                space=space,
            )
            await publish_manifest(self, ctx, manifest)
            self.snapshots[key] = snapshot
            self.generations[key] = Published(snapshot, view.commit_seq, graph_keys)
            built.append(snapshot.generation)
        return {
            "namespace_id": ctx.namespace_id,
            "indexed_through": view.commit_seq,
            "generations": built,
        }


def graph_options(graph: GraphAssets | None) -> BuildOptions:
    if graph is None:
        return {}
    return {
        "tag_vectors": graph.vectors,
        "tag_indptr": graph.member_indptr,
        "tag_indices": graph.member_indices,
        "graph_indptr": graph.edge_indptr,
        "graph_indices": graph.edge_indices,
        "graph_weights": graph.edge_weights,
        "graph_polarities": graph.edge_polarities,
        "source_references": graph.source_references,
        "candidate_source_indptr": graph.candidate_source_indptr,
        "candidate_source_indices": graph.candidate_source_indices,
        "graph_source_indptr": graph.graph_source_indptr,
        "graph_source_indices": graph.graph_source_indices,
    }
