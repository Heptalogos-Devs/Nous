"""Capture one namespace's eligible materials in a consistent read view."""

from dataclasses import dataclass

from nous.cognition.contracts import Namespace
from nous.cognition.material_contracts import MaterialRef, MaterialSnapshot
from nous.cognition.materials import Materials
from nous.cognition.scope import NamespaceOperations
from nous.common.errors import NousError
from nous.storage.database import ReadTx


@dataclass(frozen=True)
class CaptureDirectory:
    namespace: Namespace
    entries: dict[str, MaterialSnapshot]
    needs_review_count: int


@dataclass(frozen=True)
class CapturedCorpus:
    namespace: Namespace
    materials: dict[str, MaterialSnapshot]
    current: tuple[str, ...]


class CorpusCapture:
    """Apply retrieval eligibility to material facts captured by their owners."""

    def __init__(self, scope: NamespaceOperations, materials: Materials):
        self.scope, self.materials = scope, materials

    async def directory_in(
        self,
        tx: ReadTx,
        namespace: str,
        *,
        include_review: bool,
        include_exploration: bool,
        include_work: bool,
    ) -> CaptureDirectory:
        ns = await self.scope.get_in(tx, namespace)
        snapshots = await self.materials.current_in(tx, ns.ref)
        entries: dict[str, MaterialSnapshot] = {}
        groups: dict[str, MaterialRef] = {}
        review_count = 0
        enabled: set[str] = {"work"} if include_work else set()
        for snapshot in snapshots:
            view = snapshot.view
            index = snapshot.index
            if not index.available:
                continue
            if index.opt_in is not None and index.opt_in not in enabled:
                continue
            if index.needs_review:
                review_count += 1
            if index.needs_review and not include_review:
                continue
            if index.purpose == "exploration" and not include_exploration:
                continue
            if view.body is not None:
                if index.group_key in groups and groups[index.group_key] != view.ref:
                    raise NousError(
                        "material.identity_conflict",
                        "Searchable materials have conflicting identities",
                    )
                entries[index.key] = snapshot
                groups[index.group_key] = view.ref
        return CaptureDirectory(ns, entries, review_count)

    async def expand_in(
        self, tx: ReadTx, directory: CaptureDirectory, needed_keys: tuple[str, ...]
    ) -> CapturedCorpus:
        """Resolve precise source ancestors through the common material registry."""
        captured = {key: directory.entries[key] for key in needed_keys}
        frontier: list[MaterialSnapshot] = list(captured.values())
        while frontier:
            pending: dict[str, MaterialRef] = {}
            for snapshot in frontier:
                for relation in snapshot.index.support_relations:
                    key = self.materials.key(relation.target)
                    if key not in captured:
                        pending[key] = relation.target
                    for selection in relation.evidence:
                        artifact_key = self.materials.key(selection.artifact)
                        if artifact_key not in captured:
                            pending[artifact_key] = selection.artifact
            if not pending:
                break
            values = await self.materials.capture_in(
                tx, tuple(pending.values()), namespace=directory.namespace.ref
            )
            frontier = []
            for value in values:
                key = value.index.key
                if key not in captured:
                    captured[key] = value
                    frontier.append(value)
        return CapturedCorpus(directory.namespace, captured, needed_keys)
