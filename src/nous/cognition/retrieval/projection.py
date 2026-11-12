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
    Candidate,
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
    GroupDescriptor,
    PathStep,
    ReusableDocument,
    SourceSequence,
    SupportGroup,
    document_order,
)

from ..concept_contracts import AnnotationView
from .capture import CapturedCorpus
from .contracts import CognitiveGroupContext


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

    @staticmethod
    def combine(
        collection: str,
        profile: str,
        namespace_id: str,
        generation: int,
        needs_review_count: int,
        fragments: Sequence[tuple[str, str, CorpusView | ReusableDocument, GroupDescriptor]],
        sources: dict[str, ReusableDocument],
    ) -> CorpusView:
        """Assemble independent group projections without expanding unchanged sources."""
        documents: dict[str, Document] = {}
        concepts: dict[str, Concept] = {}
        candidates: dict[str, Candidate | DocumentCandidates] = {}
        associations: dict[str, Association] = {}
        support_groups: dict[str, SupportGroup] = {}
        sequences: dict[str, SourceSequence] = {}
        roots: dict[str, tuple[ArtifactRef, ...]] = {}
        memberships: dict[str, tuple[str, ...]] = {}
        source_documents: dict[str, None] = {}
        groups: list[GroupDescriptor] = []
        stamps: dict[str, str] = {}

        def merge[T](target: dict[str, T], key: str, value: T) -> None:
            if key in target and target[key] != value:
                raise NousError(
                    "wave.projection_conflict",
                    "Projection fragments disagree",
                    details={"key": key},
                )
            target[key] = value

        for value in sources.values():
            merge(documents, value.document.key, value.document)
            for key, refs in value.roots.items():
                merge(roots, key, refs)
            merge(memberships, value.document.key, value.memberships)
            if value.projection.source_document:
                source_documents[value.document.key] = None

        for document_key, stamp, fragment, group in fragments:
            if isinstance(fragment, CorpusView):
                for value in fragment.documents:
                    merge(documents, value.key, value)
                for key, refs in fragment.evidence.roots.items():
                    merge(roots, key, refs)
                for key, value in fragment.evidence.documents.items():
                    merge(memberships, key, value)
                source_documents.update(dict.fromkeys(fragment.evidence.source_documents))
                declarations = fragment
            else:
                declarations = fragment.projection
            for value in declarations.concepts:
                merge(concepts, value.key, value)
            for value in declarations.candidates:
                merge(candidates, value.key, value)
            for value in declarations.associations:
                merge(associations, value.key, value)
            for value in declarations.support_groups:
                merge(support_groups, value.key, value)
            for value in declarations.source_sequences:
                sequences.setdefault(value.model_dump_json(), value)
            groups.append(group)
            stamps[document_key] = stamp
        return CorpusView(
            name=collection,
            owner=CognitionOwner(namespace_id=namespace_id),
            generation=generation,
            profile=profile,
            metadata={"needs_review_count": needs_review_count},
            documents=tuple(documents.values()),
            groups=tuple(groups),
            evidence=EvidenceProjection(
                roots=roots, documents=memberships, source_documents=tuple(source_documents)
            ),
            concepts=tuple(concepts.values()),
            candidates=tuple(candidates.values()),
            source_sequences=tuple(sequences.values()),
            support_groups=tuple(support_groups.values()),
            associations=tuple(associations.values()),
            capture_stamps=stamps,
        )

    async def build(
        self,
        captured: CapturedCorpus,
        collection: str,
        profile: str,
        *,
        candidate_limit: int,
    ) -> CorpusView:
        """Project the captured revisions, sources and annotations for a Wave collection.

        The directory has already selected eligible revisions. Input revisions and
        their source references stay exact.
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
                    title=item.revision["title"],
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
                root = "artifact:" + origin.id
                root_refs[root] = (origin,)
                keys.append(root)
            document_roots["artifact:" + object_id] = tuple(dict.fromkeys(keys))
            if summary.support_roots == (ref,) and summary.terminal_roots == (ref,):
                source_documents.append("artifact:" + object_id)

        for key in document_order(documents):
            if key in document_roots:
                continue
            document = document_values[key]
            if document.derived_from:
                value = tuple(
                    dict.fromkeys(
                        root for parent in document.derived_from for root in document_roots[parent]
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
        evidence = EvidenceProjection(
            roots=root_refs, documents=document_roots, source_documents=tuple(source_documents)
        )
        candidates: list[DocumentCandidates] = []
        groups: list[GroupDescriptor] = []
        support_groups: list[SupportGroup] = []
        associations: list[Association] = []
        concepts: dict[str, Concept] = {}
        sequences: list[SourceSequence] = []
        source_documents_set = set(evidence.source_documents)
        for object_id in captured.current:
            item = captured.records[object_id]
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
                if doc in source_documents_set:
                    sequences.extend(
                        SourceSequence(
                            document=doc,
                            concepts=tuple(step.concept for step in path.steps),
                            ordered=True,
                        )
                        for path in paths
                    )
            context = CognitiveGroupContext(
                revision_ref=item.ref,
                kind=item.record["kind"],
                purpose=item.revision["purpose"],
                needs_review=object_id in captured.needs_review,
                sources=item.sources,
            )
            groups.append(
                GroupDescriptor(
                    key=item.ref.identifier,
                    document_key=doc,
                    title=item.revision["title"],
                    context=cast(dict[str, JsonValue], context.model_dump(mode="json")),
                )
            )
            candidates.append(
                DocumentCandidates(
                    document=doc,
                    group_key=item.ref.identifier,
                    whole=True,
                    local_layout=CompleteBlocks(target_bytes=candidate_limit),
                    paths=paths,
                    occurrences=occurrences,
                    concepts=None,
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
            groups=tuple(groups),
            concepts=tuple(concepts.values()),
            source_sequences=tuple(sequences),
            candidates=tuple(candidates),
            support_groups=tuple(support_groups),
            associations=tuple(associations),
        )
