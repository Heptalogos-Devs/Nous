from __future__ import annotations

import asyncio
import shutil
from pathlib import Path

from nous_native import IndexStore, Snapshot

from nous.cognition.models import ModelServices
from nous.cognition.service import CognitionService
from nous.material.derived import DerivedService
from nous.material.service import MaterialService
from nous.platform.persistence import Authority
from nous.references.service import ReferenceDirectory

from . import candidates, embedding, graph, query, status


class RetrievalService:
    status = status.status
    eligible = candidates.eligible
    selectors = candidates.selectors
    order = candidates.order
    graph = graph.graph
    query = query.query
    query_embedding = embedding.query_embedding
    embed_media = embedding.embed_media

    def __init__(
        self,
        authority: Authority,
        cognition: CognitionService,
        models: ModelServices,
        index_path: str | Path,
        materials: MaterialService | None = None,
        derivations: DerivedService | None = None,
    ) -> None:
        self.authority, self.cognition, self.models = authority, cognition, models
        self.materials = materials or MaterialService(authority)
        self.derivations = derivations or cognition.derivations
        self.references = ReferenceDirectory(authority, cognition, self.materials)
        self.index_path = Path(index_path)
        self.index_path.mkdir(parents=True, exist_ok=True)
        self.snapshots: dict[str, Snapshot] = {}
        self.stores: dict[str, IndexStore] = {}
        self.lock = asyncio.Lock()

    async def close(self) -> None:
        async with self.lock:
            for store in self.stores.values():
                store.close()
            self.snapshots.clear()
            self.stores.clear()

    async def invalidate(self) -> None:
        await self.close()
        await asyncio.to_thread(shutil.rmtree, self.index_path, True)
        self.index_path.mkdir(parents=True, exist_ok=True)
