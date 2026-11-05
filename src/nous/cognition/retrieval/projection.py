"""Project the selected formal corpus into exact documents, candidates and evidence."""

from collections.abc import Sequence
from typing import cast

from pydantic import JsonValue

from nous.common.errors import NousError
from nous.content.contracts import ArtifactRef
from nous.content.results import ResultStore
from nous.temporal.values import CognitiveTimes
from nous.wave.contracts import (
    Association,
    CandidateContent,
    CandidateOccurrence,
    CandidatePath,
    CognitionOwner,
    CompleteBlocks,
    Concept,
    CorpusView,
    Document,
    DocumentCandidates,
    DocumentInput,
    EvidenceProjection,
    PathStep,
    SourceSequence,
    SupportGroup,
)

from ..concept_contracts import AnnotationView
from .capture import CapturedCorpus


def annotation_paths(annotation: AnnotationView) -> tuple[CandidatePath, ...]:
    occurrences = {occurrence.key: occurrence for occurrence in annotation.occurrences}
    return tuple(
        CandidatePath(
            key=path.key,
            label=path.label,
            steps=tuple(
                PathStep(
                    concept=annotation.bindings[occurrences[key].concept_key].ref.identifier,
                    selections=tuple(part.selection for part in occurrences[key].selections),
                )
                for key in path.occurrences
            ),
        )
        for path in annotation.paths
    )


def annotation_occurrences(annotation: AnnotationView) -> tuple[CandidateOccurrence, ...]:
    return tuple(
        CandidateOccurrence(
            key=occurrence.key,
            concept=annotation.bindings[occurrence.concept_key].ref.identifier,
            selections=tuple(part.selection for part in occurrence.selections),
        )
        for occurrence in annotation.occurrences
    )


def interpretation_contributions(
    document: str, paths: Sequence[CandidatePath], basis_refs: Sequence[CandidateContent]
) -> tuple[SupportGroup | None, tuple[Association, ...]]:
    if not basis_refs:
        return None, ()
    pairs = list(
        dict.fromkeys(
            (left.concept, right.concept)
            for path in paths
            for left, right in zip(path.steps, path.steps[1:], strict=False)
            if left.concept != right.concept
        )
    )
    if not pairs:
        return None, ()
    group = SupportGroup(
        key=f"interpretation:{document}",
        kind="interpretation",
        required_sources=tuple(dict.fromkeys(ref.document for ref in basis_refs)),
        basis_refs=tuple(basis_refs),
        origin_ref=CandidateContent(document=document),
    )
    edges = tuple(
        Association(
            key=f"{group.key}:{index}",
            source=source,
            target=target,
            relation="narrative_next",
            directed=True,
            strength=1.0,
            confidence=1.0,
            polarity="positive",
            support_group=group.key,
        )
        for index, (source, target) in enumerate(pairs, 1)
    )
    return group, edges


class ProjectionBuilder:
    """Build Wave projections from a frozen cognitive corpus."""
    def __init__(self, results: ResultStore):
        self.results = results

    async def build(
        self,
        captured: CapturedCorpus,
        collection: str,
        profile: str,
        *,
        include_review: bool,
        include_exploration: bool,
        candidate_limit: int,
    ) -> CorpusView:
        """Project the captured revisions, sources and annotations for a Wave collection.

        The inclusion flags and candidate size limit determine which captured material
        enters the projection. Input revisions and their source references stay exact.
        """
        documents: list[Document] = []
        external: dict[str, ArtifactRef] = {}
        relevant: set[str] = set()
        pending = list(captured.current)
        while pending:
            object_id = pending.pop()
            if object_id in relevant:
                continue
            relevant.add(object_id)
            pending.extend(
                source.owner_revision.revision_id
                for source in captured.records[object_id].sources
                if source.usage != "inspiration" and source.owner_revision is not None
            )
        for object_id, item in captured.records.items():
            if object_id not in relevant:
                continue
            parents: list[str] = []
            for source in item.sources:
                if source.usage == "inspiration":
                    continue
                if source.owner_revision:
                    parents.append("revision:" + source.owner_revision.revision_id)
                else:
                    for selection in source.selections:
                        ref = selection.artifact
                        external[ref.id] = ref
                        parents.append("artifact:" + ref.id)
            documents.append(
                Document(
                    key="revision:" + object_id,
                    input=DocumentInput(artifact=item.body.identifier),
                    derived_from=tuple(dict.fromkeys(parents)),
                    times=CognitiveTimes.model_validate(item.revision["cognitive_times"]),
                )
            )
        documents.extend(
            Document(key="artifact:" + key, input=DocumentInput(artifact=ref.identifier))
            for key, ref in external.items()
        )
        document_values = {document.key: document for document in documents}
        root_refs: dict[str, tuple[ArtifactRef, ...]] = {}
        document_roots: dict[str, tuple[str, ...]] = {}
        source_documents: list[str] = []
        formal_origins = {
            ref.id: "record:" + object_id for object_id, ref in captured.origins.items()
        }
        external_lineage = await self.results.provenance(tuple(external.values()))
        for object_id, ref in external.items():
            summary = external_lineage[object_id]
            if not summary.complete:
                raise NousError(
                    "wave.provenance_incomplete",
                    "External source lineage is incomplete",
                    details={"artifact": ref.identifier},
                )
            keys: list[str] = []
            for origin in summary.support_roots:
                root = formal_origins.get(origin.id, "artifact:" + origin.id)
                root_refs[root] = (origin,)
                keys.append(root)
            document_roots["artifact:" + object_id] = tuple(dict.fromkeys(keys))
            if summary.support_roots == (ref,) and summary.terminal_roots == (ref,):
                source_documents.append("artifact:" + object_id)

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
        candidates: list[DocumentCandidates] = []
        support_groups: list[SupportGroup] = []
        associations: list[Association] = []
        concepts: dict[str, Concept] = {}
        sequences: list[SourceSequence] = []
        for object_id in captured.current:
            item = captured.records[object_id]
            if item.revision["purpose"] == "exploration" and not include_exploration:
                continue
            if object_id in captured.needs_review and not include_review:
                continue
            doc = "revision:" + object_id
            annotation = captured.annotations.get(object_id)
            paths = annotation_paths(annotation) if annotation else ()
            occurrences = annotation_occurrences(annotation) if annotation else ()
            if annotation:
                concepts.update(
                    {
                        meaning.ref.identifier: Concept(
                            key=meaning.ref.identifier,
                            label=meaning.label,
                            description=meaning.description,
                            aliases=meaning.aliases,
                        )
                        for meaning in annotation.bindings.values()
                    }
                )
                if doc in evidence.source_documents:
                    sequences.extend(
                        SourceSequence(
                            document=doc,
                            concepts=tuple(step.concept for step in path.steps),
                            ordered=True,
                        )
                        for path in paths
                    )
            metadata: dict[str, JsonValue] = {
                "record_revision": cast(dict[str, JsonValue], item.ref.model_dump(mode="json")),
                "title": item.revision["title"],
                "kind": item.record["kind"],
                "purpose": item.revision["purpose"],
                "annotation": annotation.ref.identifier if annotation else None,
                "needs_review": object_id in captured.needs_review,
                "sources": [source.model_dump(mode="json") for source in item.sources],
            }
            candidates.append(
                DocumentCandidates(
                    document=doc,
                    group_key=item.ref.identifier,
                    whole=True,
                    local_layout=CompleteBlocks(target_bytes=candidate_limit),
                    paths=paths,
                    occurrences=occurrences,
                    concepts=None,
                    metadata=metadata,
                )
            )
            if (
                item.record["kind"] == "journal"
                and document_values[doc].derived_from
                and evidence.documents[doc]
                and paths
            ):
                premises = tuple(
                    CandidateContent(
                        document="revision:" + source.owner_revision.revision_id
                        if source.owner_revision
                        else "artifact:" + part.artifact.id,
                        selector=part.selection,
                    )
                    for source in item.sources
                    if source.usage != "inspiration"
                    for part in source.selections
                )
                group, edges = interpretation_contributions(doc, paths, premises)
                if group is not None:
                    support_groups.append(group)
                    associations.extend(edges)
        return CorpusView(
            name=collection,
            evidence=evidence,
            owner=CognitionOwner(namespace_id=captured.namespace.id),
            generation=captured.namespace.content_generation,
            profile=profile,
            metadata={"needs_review_count": len(captured.needs_review)},
            documents=tuple(documents),
            concepts=tuple(concepts.values()),
            source_sequences=tuple(sequences),
            candidates=tuple(candidates),
            support_groups=tuple(support_groups),
            associations=tuple(associations),
        )
