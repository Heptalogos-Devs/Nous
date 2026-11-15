"""Capture one namespace's eligible materials in a consistent read view."""

from dataclasses import dataclass
from typing import TYPE_CHECKING

from pydantic import Field

from nous.cognition.basis import needs_review_in
from nous.cognition.concept_contracts import AnnotationView
from nous.cognition.contracts import CognitiveValue, Namespace, NamespaceRef
from nous.cognition.material_contracts import (
    CognitiveMaterialView,
    MaterialRelation,
)
from nous.cognition.materials import Materials
from nous.cognition.scope import NamespaceOperations
from nous.common.errors import NousError
from nous.common.material_refs import RecordRevisionRef, WorkRevisionRef
from nous.content.contracts import ArtifactRef, MaterialRef, ResultOutputRef
from nous.content.selections import subtract_selection
from nous.storage.database import ReadTx

if TYPE_CHECKING:
    from nous.cognition.annotation.operations import AnnotationOperations
    from nous.cognition.work.reading import WorkReader
    from nous.cognition.writing.reading import RecordReader
    from nous.content.artifacts import ArtifactStore
    from nous.content.results import ResultStore


class MaterialRoot(CognitiveValue):
    key: str = Field(min_length=1)
    artifact: ArtifactRef


class MaterialIndex(CognitiveValue):
    key: str = Field(min_length=1)
    group_key: str = Field(min_length=1)
    namespace: NamespaceRef | None = None
    available: bool = True
    opt_in: str | None = None
    purpose: str | None = None
    needs_review: bool = False
    annotation: AnnotationView | None = None
    interpretation: bool = False
    root: MaterialRoot | None = None
    provenance: ArtifactRef | None = None
    support_relations: tuple[MaterialRelation, ...] = ()


class MaterialSnapshot(CognitiveValue):
    view: CognitiveMaterialView
    body_digest: str | None
    index: MaterialIndex


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

    def __init__(
        self,
        scope: NamespaceOperations,
        materials: Materials,
        artifacts: ArtifactStore,
        results: ResultStore,
        records: RecordReader,
        work: WorkReader,
        annotations: AnnotationOperations,
    ):
        self.scope, self.materials = scope, materials
        self.artifacts, self.results, self.records, self.work, self.annotations = (
            artifacts,
            results,
            records,
            work,
            annotations,
        )

    async def index_in(
        self, tx: ReadTx, views: tuple[CognitiveMaterialView, ...]
    ) -> tuple[MaterialSnapshot, ...]:
        """Capture retrieval-only facts for these exact materials in the same read view."""
        records = tuple(view.ref for view in views if isinstance(view.ref, RecordRevisionRef))
        annotations = await self.annotations.read_many_in(tx, records) if records else {}
        review: set[str] = set()
        revision_ids = list(dict.fromkeys(ref.revision_id for ref in records))
        for offset in range(0, len(revision_ids), 800):
            review.update(await needs_review_in(tx, revision_ids[offset : offset + 800]))
        record_facts = await self.records.search_facts_in(tx, records)
        output_refs = tuple(view.ref for view in views if isinstance(view.ref, ResultOutputRef))
        output_mappings = await self.results.output_mappings_in(tx, output_refs)
        bodies = tuple(view.body.artifact for view in views if view.body is not None)
        blobs = await self.artifacts.content.resolve_many_in(tx, [body.id for body in bodies])
        digests = {body.id: blob.digest for body, blob in zip(bodies, blobs, strict=True)}
        support_kinds = {"reference", "current_basis", "source", "derived", "identity", "atomic"}
        snapshots: list[MaterialSnapshot] = []
        for view in views:
            ref = view.ref
            if isinstance(ref, RecordRevisionRef):
                facts = record_facts[ref.revision_id]
                annotation = annotations.get(ref.revision_id)
                needs_review = ref.revision_id in review
                index = MaterialIndex(
                    key=self.materials.key(ref),
                    group_key=ref.identifier,
                    namespace=NamespaceRef(
                        instance_id=ref.record_ref.instance_id, id=ref.record_ref.namespace_id
                    ),
                    available=facts.available,
                    purpose=facts.purpose,
                    needs_review=needs_review,
                    annotation=annotation,
                    interpretation=view.kind == "journal",
                    root=MaterialRoot(
                        key="record:" + ref.record_ref.id, artifact=facts.original_body
                    )
                    if facts.original_body
                    else None,
                    support_relations=tuple(
                        MaterialRelation(
                            kind=source.kind, target=source.material, evidence=source.selections
                        )
                        for source in view.material_sources
                        if source.kind in support_kinds
                    ),
                )
            elif isinstance(ref, WorkRevisionRef):
                index = MaterialIndex(
                    key=self.work.material_key(ref),
                    group_key=ref.identifier,
                    namespace=self.work.material_namespace(ref),
                    opt_in="work",
                    support_relations=self.work.search_support(view),
                )
            elif isinstance(ref, ArtifactRef):
                index = MaterialIndex(
                    key=self.materials.key(ref),
                    group_key=ref.identifier,
                    available=False,
                    provenance=ref,
                )
            else:
                assert isinstance(ref, ResultOutputRef) and view.body is not None
                source_relations = tuple(
                    MaterialRelation(
                        kind=source.kind, target=source.material, evidence=source.selections
                    )
                    for source in view.material_sources
                )
                covered = tuple(
                    part for source in view.material_sources for part in source.selections
                )
                mapping_relations = (
                    MaterialRelation(
                        kind=mapping.relation,
                        target=origin.artifact,
                        scope=(view.body.model_copy(update={"selection": mapping.selection}),),
                        evidence=(origin,),
                    )
                    for mapping in output_mappings.get((ref.result.id, ref.output), ())
                    for origin in mapping.origins
                )
                support = tuple(
                    item for item in source_relations if item.kind in support_kinds
                ) + tuple(
                    relation.model_copy(update={"evidence": (part,)})
                    for relation in mapping_relations
                    for origin in relation.evidence
                    for part in subtract_selection(origin, covered)
                )
                index = MaterialIndex(
                    key=self.materials.key(ref),
                    group_key=f"{ref.result.identifier}:{ref.output}",
                    available=False,
                    provenance=view.body.artifact if not view.material_sources else None,
                    support_relations=support,
                )
            snapshots.append(
                MaterialSnapshot(
                    view=view,
                    body_digest=digests.get(view.body.artifact.id) if view.body else None,
                    index=index,
                )
            )
        return tuple(snapshots)

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
        snapshots = await self.index_in(tx, await self.materials.current_in(tx, ns.ref))
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
            values = await self.index_in(
                tx,
                await self.materials.capture_in(
                    tx, tuple(pending.values()), namespace=directory.namespace.ref
                ),
            )
            frontier = []
            for value in values:
                key = value.index.key
                if key not in captured:
                    captured[key] = value
                    frontier.append(value)
        return CapturedCorpus(directory.namespace, captured, needed_keys)
