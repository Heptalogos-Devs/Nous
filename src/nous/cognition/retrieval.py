"""Capture formal written revisions and publish their current Wave view."""

from __future__ import annotations

import asyncio
from dataclasses import dataclass
from typing import Literal, cast

import sqlalchemy as sa
from pydantic import Field, JsonValue

from nous.common.errors import NousError
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ArtifactRef, ContentSelection, SourceSelection
from nous.content.results import ResultStore
from nous.content.schema import artifacts as artifact_table
from nous.content.text import TextView
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database, ReadTx, WriteTx
from nous.temporal.values import CognitiveTimes
from nous.wave.contracts import (
    Candidate,
    CandidateContent,
    CandidateFilter,
    CandidatePath,
    Concept,
    ContextFragment,
    CorpusView,
    Document,
    DocumentInput,
    EvidenceProjection,
    PathStep,
    QueryCue,
    SourceSequence,
    WaveQuery,
)
from nous.wave.operations import WaveOperations

from . import basis, concept_schema, schema
from .contracts import CognitiveValue, Namespace, RecordRevisionRef, SourceBinding
from .record_reading import RecordRow, RevisionRow, WrittenRecordReader
from .scope import NamespaceOperations

Mode = Literal["auto", "lexical", "dense", "tagmemo", "rivermemo", "hybrid", "dtsc"]
SearchMode = Literal["lexical", "dense", "tagmemo", "rivermemo", "hybrid", "dtsc"]


class CognitiveQuery(CognitiveValue):
    text: str = ""
    namespace: str | None = None
    mode: Mode | None = None
    snapshot: str | None = None
    top_k: int | None = Field(default=None, ge=1)
    include_review: bool | None = None
    include_exploration: bool | None = None
    explain: bool = False
    context: tuple[ContextFragment, ...] = ()
    core_concepts: tuple[str, ...] = ()
    cues: tuple[QueryCue, ...] = ()
    filters: CandidateFilter | None = None


@dataclass(frozen=True)
class CapturedRecord:
    record: RecordRow
    revision: RevisionRow
    ref: RecordRevisionRef
    body: ArtifactRef
    sources: tuple[SourceBinding, ...]


@dataclass(frozen=True)
class CapturedOccurrence:
    concept: str
    selection: SourceSelection


@dataclass(frozen=True)
class CapturedAnnotation:
    address: str
    concepts: dict[str, Concept]
    occurrences: dict[str, CapturedOccurrence]
    paths: tuple[CandidatePath, ...]


@dataclass(frozen=True)
class CapturedCorpus:
    namespace: Namespace
    records: dict[str, CapturedRecord]
    current: tuple[str, ...]
    annotations: dict[str, CapturedAnnotation]
    needs_review: frozenset[str]
    origins: dict[str, ArtifactRef]


@dataclass(frozen=True)
class SearchOptions:
    namespace: str
    mode: SearchMode
    space: str | None
    include_review: bool
    include_exploration: bool
    candidate_limit: int


class CognitiveRetrieval:
    """Own current-view selection; Wave owns construction and snapshot publication."""

    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        scope: NamespaceOperations,
        records: WrittenRecordReader,
        artifacts: ArtifactStore,
        results: ResultStore,
        wave: WaveOperations,
    ) -> None:
        self.database, self.execution, self.scope = database, execution, scope
        self.records, self.artifacts, self.wave = records, artifacts, wave
        self.results = results
        self._locks: dict[tuple[str, str], asyncio.Lock] = {}

    def _options(
        self,
        namespace: str | None,
        mode: Mode | None,
        include_review: bool | None,
        include_exploration: bool | None,
    ) -> SearchOptions:
        settings = self.execution.snapshot.typed_values().cognition
        selected = mode or settings.search.default_mode
        if selected == "auto":
            selected = "hybrid" if settings.search.space else "lexical"
        space = None if selected == "lexical" else settings.search.space
        if selected != "lexical" and not space:
            raise NousError(
                "cognition.space_required", "This retrieval mode requires an embedding space"
            )
        return SearchOptions(
            namespace or settings.default_namespace,
            selected,
            space,
            settings.search.include_review if include_review is None else include_review,
            settings.search.include_exploration
            if include_exploration is None
            else include_exploration,
            settings.search.local_candidate_max_bytes,
        )

    async def capture_in(self, tx: ReadTx, namespace: str) -> CapturedCorpus:
        """Copy current heads and their exact source DAG from one committed view."""
        ns = await self.scope.get_in(tx, namespace)
        current_rows = (
            (
                await tx.connection.execute(
                    sa.select(schema.records).where(
                        schema.records.c.namespace_id == ns.id,
                        schema.records.c.archived.is_(False),
                        schema.records.c.recall.is_(True),
                    )
                )
            )
            .mappings()
            .all()
        )
        current = tuple(str(row["head_revision_id"]) for row in current_rows)
        if not current:
            return CapturedCorpus(ns, {}, (), {}, frozenset(), {})

        ancestors = (
            sa.select(schema.revisions.c.id.label("id"))
            .where(schema.revisions.c.id.in_(current))
            .cte("cognition_ancestors", recursive=True)
        )
        ancestors = ancestors.union(
            sa.select(schema.sources.c.target_revision_id.label("id"))
            .join(ancestors, schema.sources.c.revision_id == ancestors.c.id)
            .where(schema.sources.c.target_revision_id.is_not(None))
        )
        revision_rows = (
            (
                await tx.connection.execute(
                    sa.select(schema.revisions).where(
                        schema.revisions.c.id.in_(sa.select(ancestors.c.id))
                    )
                )
            )
            .mappings()
            .all()
        )
        revisions = {str(row["id"]): cast(RevisionRow, dict(row)) for row in revision_rows}
        record_rows = (
            (
                await tx.connection.execute(
                    sa.select(schema.records).where(
                        schema.records.c.id.in_([row["record_id"] for row in revisions.values()])
                    )
                )
            )
            .mappings()
            .all()
        )
        records = {str(row["id"]): cast(RecordRow, dict(row)) for row in record_rows}
        source_rows = (
            (
                await tx.connection.execute(
                    sa.select(schema.sources)
                    .where(schema.sources.c.revision_id.in_(list(revisions)))
                    .order_by(schema.sources.c.revision_id, schema.sources.c.ordinal)
                )
            )
            .mappings()
            .all()
        )
        # A formal original keeps its first accepted body as the immutable witness
        # while each later revision supplies only the current text and graph.
        origin_rows = (
            (
                await tx.connection.execute(
                    sa.select(
                        schema.revisions.c.record_id, schema.revisions.c.body_artifact_id
                    ).where(
                        schema.revisions.c.record_id.in_(list(records)),
                        schema.revisions.c.ordinal == 1,
                        ~sa.select(1)
                        .where(schema.sources.c.revision_id == schema.revisions.c.id)
                        .exists(),
                    )
                )
            )
            .mappings()
            .all()
        )
        artifact_ids = {row["body_artifact_id"] for row in revisions.values()}
        artifact_ids.update(row["body_artifact_id"] for row in origin_rows)
        artifact_ids.update(str(row["artifact_id"]) for row in source_rows)
        artifact_rows = (
            (
                await tx.connection.execute(
                    sa.select(artifact_table.c.id, artifact_table.c.address).where(
                        artifact_table.c.id.in_(artifact_ids)
                    )
                )
            )
            .mappings()
            .all()
        )
        artifacts = {
            str(row["id"]): ArtifactRef(
                id=str(row["id"]),
                address=str(row["address"]),
                instance_id=self.artifacts.instance_id,
            )
            for row in artifact_rows
        }
        sources: dict[str, list[SourceBinding]] = {key: [] for key in revisions}
        for row in source_rows:
            target = revisions.get(str(row["target_revision_id"]))
            owner = (
                self.records.revision_ref(records[target["record_id"]], target) if target else None
            )
            sources[str(row["revision_id"])].append(
                SourceBinding(
                    key=row["key"],
                    content_selection=ContentSelection(
                        artifact=artifacts[str(row["artifact_id"])],
                        selection=SourceSelection.model_validate(row["selection"]),
                    ),
                    owner_revision=owner,
                    usage=row["usage"],
                    name=row["name"],
                )
            )
        captured: dict[str, CapturedRecord] = {}
        for identifier, revision in revisions.items():
            record = records[revision["record_id"]]
            captured[identifier] = CapturedRecord(
                record=record,
                revision=revision,
                ref=self.records.revision_ref(record, revision),
                body=artifacts[revision["body_artifact_id"]],
                sources=tuple(sources[identifier]),
            )
        annotations = await self._annotations_in(tx, current)
        needs_review = await basis.needs_review_in(tx, current)
        origins = {
            str(row["record_id"]): artifacts[str(row["body_artifact_id"])] for row in origin_rows
        }
        return CapturedCorpus(ns, captured, current, annotations, frozenset(needs_review), origins)

    async def _annotations_in(
        self, tx: ReadTx, current: tuple[str, ...]
    ) -> dict[str, CapturedAnnotation]:
        heads = (
            (
                await tx.connection.execute(
                    sa.select(
                        concept_schema.annotation_heads.c.record_revision_id,
                        concept_schema.annotations.c.id,
                        concept_schema.annotations.c.address,
                    )
                    .join(
                        concept_schema.annotations,
                        concept_schema.annotations.c.id
                        == concept_schema.annotation_heads.c.annotation_id,
                    )
                    .where(concept_schema.annotation_heads.c.record_revision_id.in_(current))
                )
            )
            .mappings()
            .all()
        )
        if not heads:
            return {}
        identifiers = [str(row["id"]) for row in heads]
        bindings = (
            (
                await tx.connection.execute(
                    sa.select(
                        concept_schema.bindings.c.annotation_id,
                        concept_schema.bindings.c.local_key,
                        concept_schema.concepts.c.address,
                        concept_schema.concept_revisions.c.ordinal,
                        concept_schema.concept_revisions.c.label,
                        concept_schema.concept_revisions.c.description,
                        concept_schema.concept_revisions.c.aliases,
                    )
                    .join(
                        concept_schema.concept_revisions,
                        concept_schema.concept_revisions.c.id
                        == concept_schema.bindings.c.concept_revision_id,
                    )
                    .join(
                        concept_schema.concepts,
                        concept_schema.concepts.c.id
                        == concept_schema.concept_revisions.c.concept_id,
                    )
                    .where(concept_schema.bindings.c.annotation_id.in_(identifiers))
                )
            )
            .mappings()
            .all()
        )
        occurrence_rows = (
            (
                await tx.connection.execute(
                    sa.select(concept_schema.occurrences)
                    .where(concept_schema.occurrences.c.annotation_id.in_(identifiers))
                    .order_by(concept_schema.occurrences.c.ordinal)
                )
            )
            .mappings()
            .all()
        )
        block_rows = (
            (
                await tx.connection.execute(
                    sa.select(concept_schema.blocks)
                    .where(concept_schema.blocks.c.annotation_id.in_(identifiers))
                    .order_by(concept_schema.blocks.c.ordinal)
                )
            )
            .mappings()
            .all()
        )
        path_rows = (
            (
                await tx.connection.execute(
                    sa.select(concept_schema.paths)
                    .where(concept_schema.paths.c.annotation_id.in_(identifiers))
                    .order_by(concept_schema.paths.c.ordinal)
                )
            )
            .mappings()
            .all()
        )
        steps = (
            (
                await tx.connection.execute(
                    sa.select(concept_schema.path_steps)
                    .where(concept_schema.path_steps.c.annotation_id.in_(identifiers))
                    .order_by(concept_schema.path_steps.c.ordinal)
                )
            )
            .mappings()
            .all()
        )
        output: dict[str, CapturedAnnotation] = {}
        for head in heads:
            identifier = str(head["id"])
            mapped = {
                str(row["local_key"]): Concept(
                    key=f"{row['address']}@{row['ordinal']}",
                    label=row["label"],
                    description=row["description"],
                    aliases=tuple(row["aliases"]),
                )
                for row in bindings
                if row["annotation_id"] == identifier
            }
            occurrences: dict[str, CapturedOccurrence] = {}
            for row in occurrence_rows:
                if row["annotation_id"] != identifier:
                    continue
                parts = [
                    ContentSelection.model_validate(block["selection"]).selection
                    for block in block_rows
                    if block["annotation_id"] == identifier
                    and block["occurrence_key"] == row["key"]
                ]
                starts = [part.start_byte for part in parts if part.start_byte is not None]
                ends = [part.end_byte for part in parts if part.end_byte is not None]
                if not starts or not ends:
                    raise NousError(
                        "cognition.annotation_invalid", "Annotation has no exact block selection"
                    )
                occurrences[str(row["key"])] = CapturedOccurrence(
                    concept=mapped[str(row["concept_key"])].key,
                    selection=SourceSelection(start_byte=min(starts), end_byte=max(ends)),
                )
            paths = tuple(
                CandidatePath(
                    key=str(row["key"]),
                    label=str(row["label"]),
                    steps=tuple(
                        PathStep(
                            concept=occurrences[str(step["occurrence_key"])].concept,
                            selection=occurrences[str(step["occurrence_key"])].selection,
                        )
                        for step in steps
                        if step["annotation_id"] == identifier and step["path_key"] == row["key"]
                    ),
                )
                for row in path_rows
                if row["annotation_id"] == identifier
            )
            output[str(head["record_revision_id"])] = CapturedAnnotation(
                address=str(head["address"]), concepts=mapped, occurrences=occurrences, paths=paths
            )
        return output

    @staticmethod
    def _selections(view: TextView, limit: int) -> list[SourceSelection]:
        """Group complete blocks and leave an oversized block intact."""
        groups: list[SourceSelection] = []
        start: int | None = None
        end: int | None = None
        for block in view.blocks:
            first, last = block.selection.selection.start_byte, block.selection.selection.end_byte
            assert first is not None and last is not None
            if start is not None and last - start > limit:
                groups.append(SourceSelection(start_byte=start, end_byte=end))
                start = None
            if start is None:
                start = first
            end = last
        if start is not None:
            groups.append(SourceSelection(start_byte=start, end_byte=end))
        return groups

    async def _view(
        self,
        captured: CapturedCorpus,
        collection: str,
        profile: str,
        *,
        include_review: bool,
        include_exploration: bool,
        candidate_limit: int,
    ) -> CorpusView:
        documents: list[Document] = []
        external: dict[str, ArtifactRef] = {}
        relevant: set[str] = set()
        pending = list(captured.current)
        while pending:
            identifier = pending.pop()
            if identifier in relevant:
                continue
            relevant.add(identifier)
            pending.extend(
                source.owner_revision.revision_id
                for source in captured.records[identifier].sources
                if source.usage != "inspiration" and source.owner_revision is not None
            )
        for identifier, item in captured.records.items():
            if identifier not in relevant:
                continue
            parents: list[str] = []
            for source in item.sources:
                if source.usage == "inspiration":
                    continue
                if source.owner_revision:
                    parents.append("revision:" + source.owner_revision.revision_id)
                else:
                    ref = source.content_selection.artifact
                    external[ref.id] = ref
                    parents.append("artifact:" + ref.id)
            documents.append(
                Document(
                    key="revision:" + identifier,
                    input=DocumentInput(artifact=item.body.address),
                    derived_from=tuple(dict.fromkeys(parents)),
                    times=CognitiveTimes.model_validate(item.revision["cognitive_times"]),
                )
            )
        documents.extend(
            Document(key="artifact:" + key, input=DocumentInput(artifact=ref.address))
            for key, ref in external.items()
        )
        document_values = {document.key: document for document in documents}
        root_refs: dict[str, tuple[ArtifactRef, ...]] = {}
        document_roots: dict[str, tuple[str, ...]] = {}
        source_documents: list[str] = []
        formal_origins = {
            ref.id: "record:" + identifier for identifier, ref in captured.origins.items()
        }
        external_lineage = await self.results.provenance(tuple(external.values()))
        for identifier, ref in external.items():
            summary = external_lineage[identifier]
            if not summary.complete:
                raise NousError(
                    "wave.provenance_incomplete",
                    "External source lineage is incomplete",
                    details={"artifact": ref.address},
                )
            keys: list[str] = []
            for origin in summary.support_roots:
                root = formal_origins.get(origin.id, "artifact:" + origin.id)
                root_refs[root] = (origin,)
                keys.append(root)
            document_roots["artifact:" + identifier] = tuple(dict.fromkeys(keys))
            if summary.support_roots == (ref,) and summary.terminal_roots == (ref,):
                source_documents.append("artifact:" + identifier)

        def roots(key: str) -> tuple[str, ...]:
            if key in document_roots:
                return document_roots[key]
            document = document_values[key]
            if document.derived_from:
                value = tuple(
                    dict.fromkeys(
                        root for parent in document.derived_from for root in roots(parent)
                    )
                )
            else:
                item = captured.records[key.removeprefix("revision:")]
                origin = captured.origins.get(item.record["id"])
                if origin is None:
                    value = ()
                else:
                    root = "record:" + item.record["id"]
                    root_refs[root] = (origin,)
                    value = (root,)
                    source_documents.append(key)
            document_roots[key] = value
            return value

        for document in documents:
            roots(document.key)
        evidence = EvidenceProjection(
            roots=root_refs, documents=document_roots, source_documents=tuple(source_documents)
        )
        candidates: list[Candidate] = []
        concepts: dict[str, Concept] = {}
        sequences: list[SourceSequence] = []
        for identifier in captured.current:
            item = captured.records[identifier]
            if item.revision["purpose"] == "exploration" and not include_exploration:
                continue
            if identifier in captured.needs_review and not include_review:
                continue
            doc = "revision:" + identifier
            text_view = await self.artifacts.text_view(item.body)
            selections = [SourceSelection(), *self._selections(text_view, candidate_limit)]
            annotation = captured.annotations.get(identifier)
            if annotation:
                concepts.update({concept.key: concept for concept in annotation.concepts.values()})
                if doc in evidence.source_documents:
                    sequences.extend(
                        SourceSequence(
                            document=doc,
                            concepts=tuple(step.concept for step in path.steps),
                            ordered=True,
                        )
                        for path in annotation.paths
                    )
            metadata: dict[str, JsonValue] = {
                "record_revision": cast(dict[str, JsonValue], item.ref.model_dump(mode="json")),
                "title": item.revision["title"],
                "kind": item.record["kind"],
                "purpose": item.revision["purpose"],
                "annotation": annotation.address if annotation else None,
                "needs_review": identifier in captured.needs_review,
                "sources": [
                    {
                        "key": source.key,
                        "name": source.name,
                        "revision": (
                            source.owner_revision.address
                            if source.owner_revision
                            else source.content_selection.artifact.address
                        ),
                        "usage": source.usage,
                    }
                    for source in item.sources
                ],
            }
            for index, selection in enumerate(selections):
                memberships = ()
                if annotation:
                    memberships = tuple(
                        dict.fromkeys(
                            occurrence.concept
                            for occurrence in annotation.occurrences.values()
                            if selection.is_whole
                            or (
                                selection.start_byte is not None
                                and selection.end_byte is not None
                                and occurrence.selection.start_byte is not None
                                and occurrence.selection.end_byte is not None
                                and selection.start_byte <= occurrence.selection.start_byte
                                and occurrence.selection.end_byte <= selection.end_byte
                            )
                        )
                    )
                candidates.append(
                    Candidate(
                        key=f"{identifier}:{index}",
                        document=doc,
                        content=CandidateContent(document=doc, selector=selection),
                        concepts=memberships,
                        paths=annotation.paths if annotation else (),
                        metadata=metadata,
                    )
                )
        return CorpusView(
            name=collection,
            evidence=evidence,
            generation=captured.namespace.content_generation,
            profile=profile,
            metadata={"needs_review_count": len(captured.needs_review)},
            documents=tuple(documents),
            concepts=tuple(concepts.values()),
            source_sequences=tuple(sequences),
            candidates=tuple(candidates),
        )

    async def _current_view(
        self,
        namespace: str,
        space: str | None,
        *,
        include_review: bool,
        include_exploration: bool,
        candidate_limit: int,
    ) -> tuple[Namespace, dict[str, object]]:
        policy: dict[str, object] = {
            "include_review": include_review,
            "include_exploration": include_exploration,
            "local_candidate_max_bytes": candidate_limit,
        }
        profile = self.wave.profile(space, policy)
        async with self.database.read() as tx:
            ns = await self.scope.get_in(tx, namespace)
        collection = f"cognition:{ns.id}:{profile}"
        async with self._locks.setdefault((ns.id, profile), asyncio.Lock()):
            async with self.database.read() as tx:
                captured = await self.capture_in(tx, ns.id)
            existing = await self.wave.current(collection)
            if existing and existing["covered_generation"] == captured.namespace.content_generation:
                return captured.namespace, existing
            view = await self._view(
                captured,
                collection,
                profile,
                include_review=include_review,
                include_exploration=include_exploration,
                candidate_limit=candidate_limit,
            )

            async def unchanged(tx: WriteTx) -> bool:
                current = await self.scope.get_in(tx, ns.id)
                return current.content_generation == captured.namespace.content_generation

            try:
                published = await self.wave.build_view(view, space, publish_if=unchanged)
            except NousError as error:
                raise NousError(
                    "cognition.projection_failed",
                    "The current retrieval view could not be built",
                    details={
                        "namespace": ns.name,
                        "generation": captured.namespace.content_generation,
                        "cause": error.to_dict(),
                        "retry": "journal refresh",
                    },
                    retryable=error.retryable,
                    cause=error,
                ) from error
            return captured.namespace, published

    async def refresh(
        self,
        namespace: str | None = None,
        *,
        mode: Mode | None = None,
        include_review: bool | None = None,
        include_exploration: bool | None = None,
    ) -> dict[str, object]:
        async with self.execution.operation("cognition.refresh"):
            selected = self._options(namespace, mode, include_review, include_exploration)
            ns, published = await self._current_view(
                selected.namespace,
                selected.space,
                include_review=selected.include_review,
                include_exploration=selected.include_exploration,
                candidate_limit=selected.candidate_limit,
            )
            return {"namespace": ns.model_dump(mode="json"), **published}

    async def status(self) -> dict[str, object]:
        """Report content and published generations without rebuilding or calling models."""
        async with self.execution.operation("cognition.retrieval_status"):
            failure = None
            try:
                selected = self._options(None, None, None, None)
            except NousError as error:
                failure = error.to_dict()
                settings = self.execution.snapshot.typed_values().cognition
                selected = SearchOptions(
                    settings.default_namespace,
                    settings.search.default_mode
                    if settings.search.default_mode != "auto"
                    else "hybrid",
                    settings.search.space,
                    settings.search.include_review,
                    settings.search.include_exploration,
                    settings.search.local_candidate_max_bytes,
                )
            profile = (
                None
                if failure
                else self.wave.profile(
                    selected.space,
                    {
                        "include_review": selected.include_review,
                        "include_exploration": selected.include_exploration,
                        "local_candidate_max_bytes": selected.candidate_limit,
                    },
                )
            )
            namespaces = await self.scope.list()
            states: list[dict[str, object]] = []
            async with self.database.read() as tx:
                for ns in namespaces:
                    heads = list(
                        (
                            await tx.connection.scalars(
                                sa.select(schema.records.c.head_revision_id).where(
                                    schema.records.c.namespace_id == ns.id,
                                    schema.records.c.archived.is_(False),
                                    schema.records.c.recall.is_(True),
                                )
                            )
                        ).all()
                    )
                    needs_review = len(await basis.needs_review_in(tx, heads))
                    states.append(
                        {
                            "namespace": ns.name,
                            "namespace_id": ns.id,
                            "content_generation": ns.content_generation,
                            "needs_review": needs_review,
                        }
                    )
            for state in states:
                current = (
                    await self.wave.current(f"cognition:{state['namespace_id']}:{profile}")
                    if profile is not None
                    else None
                )
                covered = current["covered_generation"] if current else None
                state["covered_generation"] = covered
                state["snapshot"] = current["snapshot"] if current else None
                state["pending"] = (
                    state["content_generation"] != covered
                    if covered is not None
                    else state["content_generation"] != 0
                )
            return {
                "profile": profile,
                "mode": selected.mode,
                "namespaces": states,
                "state": "unconfigured" if failure else "ready",
                "error": failure,
            }

    async def query(self, request: CognitiveQuery) -> dict[str, object]:
        async with self.execution.operation("cognition.query"):
            selected = self._options(
                request.namespace, request.mode, request.include_review, request.include_exploration
            )
            if request.snapshot:
                async with self.database.read() as tx:
                    ns = await self.scope.get_in(tx, selected.namespace)
                info = await self.wave.inspect(request.snapshot)
                if not str(info["collection"]).startswith(f"cognition:{ns.id}:"):
                    raise NousError(
                        "cognition.namespace_mismatch", "Snapshot belongs to another namespace"
                    )
                snapshot = cast(dict[str, str], info["snapshot"])["address"]
                generation = info["covered_generation"]
                view_metadata = info["metadata"]
            else:
                ns, published = await self._current_view(
                    selected.namespace,
                    selected.space,
                    include_review=selected.include_review,
                    include_exploration=selected.include_exploration,
                    candidate_limit=selected.candidate_limit,
                )
                snapshot = cast(dict[str, str], published["snapshot"])["address"]
                generation = published["covered_generation"]
                view_metadata = published["metadata"]
            limit = request.top_k or self.execution.snapshot.typed_values().wave.query.top_k
            result = await self.wave.query(
                WaveQuery(
                    snapshot=snapshot,
                    text=request.text,
                    mode=selected.mode,
                    top_k=limit * 4,
                    include_trace=request.explain,
                    context=request.context,
                    core_concepts=request.core_concepts,
                    cues=request.cues,
                    candidate_filter=request.filters,
                )
            )
            grouped: list[dict[str, object]] = []
            seen: set[str] = set()
            for hit in cast(list[dict[str, object]], result["hits"]):
                hit_metadata = cast(dict[str, object], hit["metadata"])
                ref = RecordRevisionRef.model_validate(hit_metadata["record_revision"])
                if ref.record_ref.id in seen:
                    continue
                seen.add(ref.record_ref.id)
                grouped.append(
                    {
                        **hit,
                        "rank": len(grouped) + 1,
                        "record_revision": ref.model_dump(mode="json"),
                        "revision": ref.address,
                        "title": hit_metadata["title"],
                        "source_summary": hit_metadata["sources"],
                        "needs_review": hit_metadata["needs_review"],
                    }
                )
                if len(grouped) >= limit:
                    break
            result["hits"] = grouped
            result["namespace"] = ns.model_dump(mode="json")
            result["query"] = request.text
            result["view_generation"] = generation
            result["needs_review_count"] = cast(dict[str, JsonValue], view_metadata).get(
                "needs_review_count", 0
            )
            return result
