from __future__ import annotations

import asyncio
from pathlib import Path
from typing import TYPE_CHECKING

from nous_native import IndexStore, Snapshot
from nous_sdk.contracts import CallContext, DomainError

from nous.cognition.models import ModelServices
from nous.cognition.records import CognitiveRecords
from nous.identity.access import require_namespace_in
from nous.material.derived import DerivedService
from nous.material.service import MaterialService
from nous.platform.configuration.service import ConfigurationService
from nous.platform.persistence import Authority
from nous.references.service import ReferenceDirectory

from . import binding, candidates, embedding, generations, graph, query, status
from .native import run_native
from .readout import finalize
from .source_expansion import expand_sources

if TYPE_CHECKING:
    from nous.data_management.resources import ResourceService
    from nous.vector_projection.service import VectorProjectionService


class RetrievalService:
    finalize = finalize
    expand_sources = expand_sources
    status = status.status
    eligible = candidates.eligible
    selectors = binding.selectors
    order = candidates.order
    graph = staticmethod(graph.graph)
    query = query.query
    query_embedding = embedding.query_embedding
    embed_media = embedding.embed_media

    def __init__(
        self,
        authority: Authority,
        cognition: CognitiveRecords,
        models: ModelServices,
        index_path: str | Path,
        materials: MaterialService,
        derivations: DerivedService,
        *,
        configuration: ConfigurationService,
        resources: ResourceService,
        projection: VectorProjectionService,
    ) -> None:
        self.authority, self.cognition, self.models = authority, cognition, models
        self.configuration = configuration
        self.materials = materials
        self.derivations = derivations
        self.references = ReferenceDirectory(
            authority, cognition, self.materials, resources=resources
        )
        self.index_path = Path(index_path)
        self.index_path.mkdir(parents=True, exist_ok=True)
        self.snapshots: dict[str, Snapshot] = {}
        self.stores: dict[str, IndexStore] = {}
        self.lock = asyncio.Lock()
        self.generations: dict[str, generations.Published] = {}
        self.build_locks: dict[str, asyncio.Lock] = {}
        self.projection = projection
        self.resources = resources

    async def rebuild_namespace(
        self, namespace_id: str, through_commit_seq: int
    ) -> dict[str, object]:
        # This identity is created by the index owner for one namespace. Job
        # admission carries no user credential or mutable source selection.
        identity = CallContext(
            caller_id="nous.retrieval.index-reader",
            namespace_id=namespace_id,
            credential_source_ids=None,
        )
        async with self.authority.read() as view:
            await require_namespace_in(view, identity)
            if through_commit_seq < 0 or through_commit_seq > view.commit_seq:
                raise DomainError(
                    "index_watermark_invalid",
                    "The admitted index watermark is outside committed authority",
                )
        return await generations.rebuild(self, identity)

    async def close(self) -> None:
        async with self.lock:
            stores = tuple(self.stores.values())
            self.generations.clear()
            self.snapshots.clear()
            self.stores.clear()
            await run_native("close", None, lambda: close_stores(stores))

    async def invalidate(self) -> None:
        # Existing queries own immutable snapshots; publication replaces readers.
        async with self.lock:
            self.generations.clear()
            self.snapshots.clear()


def close_stores(stores: tuple[IndexStore, ...]) -> None:
    errors: list[Exception] = []
    for store in stores:
        try:
            store.close()
        except Exception as error:
            errors.append(error)
    if errors:
        raise ExceptionGroup("Failed to close native retrieval indexes", errors)
