"""Decode the known NetworkX LPMM archive into the ordinary GraphML path."""

from __future__ import annotations

import io
import json
import pickle
import pickletools
from collections.abc import Hashable
from hashlib import sha256
from typing import TYPE_CHECKING, cast

import networkx as nx
from networkx.classes import coreviews, reportviews
from nous_sdk.contracts import BlobDocument, CallContext, DomainError
from pydantic import JsonValue, TypeAdapter

from nous.platform.concurrency import owned_thread
from nous.platform.content import ContentObject

if TYPE_CHECKING:
    from nous.material.service import MaterialService
    from nous.platform.content import ContentStore
    from nous.platform.persistence import Authority


_GRAPHS: dict[tuple[str, str], object] = {
    (cls.__module__, cls.__name__): cls
    for cls in (nx.Graph, nx.DiGraph, nx.MultiGraph, nx.MultiDiGraph)
}
_VIEWS: dict[tuple[str, str], object] = {
    (cls.__module__, cls.__name__): cls
    for cls in (
        coreviews.AtlasView,
        coreviews.AdjacencyView,
        coreviews.MultiAdjacencyView,
        reportviews.NodeView,
        reportviews.NodeDataView,
        reportviews.EdgeView,
        reportviews.OutEdgeView,
        reportviews.InEdgeView,
        reportviews.MultiEdgeView,
        reportviews.OutMultiEdgeView,
        reportviews.InMultiEdgeView,
        reportviews.DegreeView,
        reportviews.DiDegreeView,
        reportviews.InDegreeView,
        reportviews.OutDegreeView,
        reportviews.MultiDegreeView,
        reportviews.DiMultiDegreeView,
        reportviews.InMultiDegreeView,
        reportviews.OutMultiDegreeView,
    )
}
_GLOBALS = {**_GRAPHS, **_VIEWS, ("builtins", "set"): set, ("builtins", "frozenset"): frozenset}


class GraphUnpickler(pickle.Unpickler):
    def find_class(self, module: str, name: str) -> object:
        value = _GLOBALS.get((module, name))
        if value is None:
            raise ValueError(f"Unsupported LPMM graph global: {module}.{name}")
        return value

    def persistent_load(self, pid: object) -> object:
        raise ValueError("External pickle objects are not part of an LPMM graph")


def to_graphml(raw: bytes) -> bytes:
    for opcode, _, _ in pickletools.genops(raw):
        if opcode.name in {"EXT1", "EXT2", "EXT4"}:
            raise ValueError("Pickle extension globals are not part of an LPMM graph")
    decoded: object = GraphUnpickler(io.BytesIO(raw)).load()
    if type(decoded) not in (nx.Graph, nx.DiGraph, nx.MultiGraph, nx.MultiDiGraph):
        raise ValueError("LPMM graph archive must contain a NetworkX graph")
    graph = cast("nx.Graph[Hashable]", decoded)
    # Only the graph's actual public data enters GraphML. Runtime caches and
    # Python object state stay in the immutable original archive.
    clean: nx.Graph[str | int] = (
        nx.MultiDiGraph()
        if graph.is_multigraph() and graph.is_directed()
        else nx.MultiGraph()
        if graph.is_multigraph()
        else nx.DiGraph()
        if graph.is_directed()
        else nx.Graph()
    )

    def attributes(value: object) -> dict[str, str | int | float | bool]:
        data = TypeAdapter(dict[str, JsonValue]).validate_python(value)
        return {
            key: item
            if isinstance(item, (str, int, float, bool))
            else json.dumps(item, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
            for key, item in data.items()
        }

    clean.graph.update(attributes(graph.graph))
    for node, data in graph.nodes(data=True):
        if not isinstance(node, (str, int)) or isinstance(node, bool):
            raise ValueError(
                "LPMM graph nodes require their original string or integer identifiers"
            )
        clean.add_node(node, **attributes(data))
    if graph.is_multigraph():
        multigraph = cast("nx.MultiGraph[Hashable]", graph)
        target_graph = cast("nx.MultiGraph[str | int]", clean)
        for source, target, key, data in multigraph.edges(keys=True, data=True):
            if not isinstance(key, (str, int)) or isinstance(key, bool):
                raise ValueError("LPMM multigraph edge keys require string or integer identifiers")
            _ = target_graph.add_edge(
                cast(str | int, source), cast(str | int, target), key=key, **attributes(data)
            )
    else:
        for source, target, data in graph.edges(data=True):
            clean.add_edge(cast(str | int, source), cast(str | int, target), **attributes(data))
    stream = io.BytesIO()
    nx.write_graphml_xml(clean, stream, encoding="utf-8")
    return stream.getvalue()


async def convert_lpmm_graph(
    ctx: CallContext,
    material_id: str,
    *,
    authority: Authority,
    materials: MaterialService,
    content: ContentStore,
) -> str:
    material = await materials.read(ctx, material_id)
    async with content.protect(
        ContentObject(material["content_hash"], material["byte_len"], material["media_type"])
    ) as path:
        try:
            converted = await owned_thread(lambda: to_graphml(path.read_bytes()))
        except (
            ValueError,
            TypeError,
            AttributeError,
            KeyError,
            RecursionError,
            nx.NetworkXException,
            pickle.UnpicklingError,
            EOFError,
        ) as error:
            raise DomainError("lpmm_graph_invalid", str(error)) from error
    lease = await content.publish_bytes(converted, "application/graphml+xml")
    try:
        operation = (
            "lpmm-graphml:"
            + sha256((material_id + ":" + material["content_hash"]).encode()).hexdigest()
        )
        async with authority.write(
            ctx,
            operation,
            operation_kind="imports.lpmm_graph",
            scope=ctx.namespace_id,
            payload={"material_id": material_id},
        ) as tx:
            if tx.replayed:
                identifier = str(tx.replay_result)
                await materials.read_in(tx, ctx, identifier)
                return identifier
            document = BlobDocument(
                filename="graph.graphml",
                media_type="application/graphml+xml",
                source_type="source_graph",
                origin={
                    "converted_from_material_id": material_id,
                    "implementation": "networkx_lpmm_graphml",
                },
            )
            result = await materials.put_blob_in(
                tx, ctx, lease, document, required_source_ids=(material_id,)
            )
            await materials.observe_in(tx, ctx, result["id"], document)
            return tx.result(result["id"])
    finally:
        await content.release(lease)
