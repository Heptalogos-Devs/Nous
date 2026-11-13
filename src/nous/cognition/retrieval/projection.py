"""Project the selected formal corpus into exact documents, candidates and evidence."""

from collections.abc import Sequence
from typing import cast

from pydantic import JsonValue

from nous.common.errors import NousError
from nous.content.contracts import ArtifactRef
from nous.content.results import ResultStore
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
from ..materials import Materials
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

    def __init__(self, results: ResultStore, materials: Materials):
        self.results, self.materials = results, materials

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
        self, captured: CapturedCorpus, collection: str, profile: str, *, candidate_limit: int
    ) -> CorpusView:
        """Project frozen common material views and their exact source relations."""
        views = {key: item.view for key, item in captured.materials.items()}
        documents: list[Document] = []
        for key, view in views.items():
            if view.body is None:
                raise NousError("wave.projection_incomplete", "Captured material has no body")
            parents = tuple(
                dict.fromkeys(
                    self.materials.key(relation.target)
                    for relation in captured.materials[key].index.support_relations
                    if self.materials.key(relation.target) != key
                )
            )
            documents.append(
                Document(
                    key=key,
                    input=DocumentInput(artifact=view.body.artifact.identifier),
                    title=view.title,
                    derived_from=parents,
                    times=view.times,
                )
            )
        document_values = {document.key: document for document in documents}
        external = {
            key: item.index.provenance
            for key, item in captured.materials.items()
            if item.index.provenance is not None
        }
        lineage = await self.results.provenance(tuple(external.values()))
        roots: dict[str, tuple[ArtifactRef, ...]] = {}
        memberships: dict[str, tuple[str, ...]] = {}
        source_documents: list[str] = []
        for key in document_order(documents):
            document = document_values[key]
            index = captured.materials[key].index
            if document.derived_from:
                memberships[key] = tuple(
                    dict.fromkeys(
                        root for parent in document.derived_from for root in memberships[parent]
                    )
                )
            elif index.root is not None:
                roots[index.root.key] = (index.root.artifact,)
                memberships[key] = (index.root.key,)
                source_documents.append(key)
            elif index.provenance is not None:
                artifact = external[key]
                summary = lineage[artifact.id]
                if not summary.complete:
                    raise NousError(
                        "wave.provenance_incomplete",
                        "External source lineage is incomplete",
                        details={"artifact": artifact.identifier},
                    )
                items: list[str] = []
                for origin in summary.support_roots:
                    root = "artifact:" + origin.id
                    roots[root] = (origin,)
                    items.append(root)
                memberships[key] = tuple(dict.fromkeys(items))
                if summary.support_roots == (artifact,) and summary.terminal_roots == (artifact,):
                    source_documents.append(key)
            else:
                memberships[key] = ()
        evidence = EvidenceProjection(
            roots=roots, documents=memberships, source_documents=tuple(source_documents)
        )
        groups: list[GroupDescriptor] = []
        candidates: list[DocumentCandidates] = []
        concepts: dict[str, Concept] = {}
        sequences: list[SourceSequence] = []
        supports: list[SupportGroup] = []
        associations: list[Association] = []
        review_count = 0
        for key in captured.current:
            view = views[key]
            index = captured.materials[key].index
            annotation = index.annotation
            review_count += int(index.needs_review)
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
                if key in source_documents:
                    sequences.extend(
                        SourceSequence(
                            document=key,
                            concepts=tuple(step.concept for step in path.steps),
                            ordered=True,
                        )
                        for path in paths
                    )
            context = CognitiveGroupContext(
                material=view.ref,
                kind=view.kind,
                purpose=index.purpose,
                needs_review=index.needs_review,
                sources=tuple(
                    dict.fromkeys(
                        part
                        for relation in view.relations
                        if relation.kind != "representation"
                        for part in relation.evidence
                    )
                ),
                attributes=view.attributes,
            )
            groups.append(
                GroupDescriptor(
                    key=index.group_key,
                    document_key=key,
                    title=view.title,
                    context=cast(dict[str, JsonValue], context.model_dump(mode="json")),
                )
            )
            candidates.append(
                DocumentCandidates(
                    document=key,
                    group_key=index.group_key,
                    whole=True,
                    local_layout=CompleteBlocks(target_bytes=candidate_limit),
                    paths=paths,
                    occurrences=occurrences,
                    concepts=None,
                )
            )
            if (
                index.interpretation
                and document_values[key].derived_from
                and memberships[key]
                and paths
            ):
                premises: list[CandidateContent] = []
                for relation in index.support_relations:
                    target_key = self.materials.key(relation.target)
                    target = views[target_key]
                    if not relation.evidence:
                        premises.append(CandidateContent(document=target_key))
                    for part in relation.evidence:
                        part_key = (
                            target_key
                            if target.body and target.body.artifact == part.artifact
                            else self.materials.key(part.artifact)
                        )
                        premises.append(
                            CandidateContent(document=part_key, selector=part.selection)
                        )
                group, edges = interpretation_contributions(key, paths, tuple(premises))
                if group is not None:
                    supports.append(group)
                    associations.extend(edges)
        return CorpusView(
            name=collection,
            evidence=evidence,
            owner=CognitionOwner(namespace_id=captured.namespace.id),
            generation=captured.namespace.content_generation,
            profile=profile,
            metadata={"needs_review_count": review_count},
            documents=tuple(documents),
            groups=tuple(groups),
            concepts=tuple(concepts.values()),
            source_sequences=tuple(sequences),
            candidates=tuple(candidates),
            support_groups=tuple(supports),
            associations=tuple(associations),
        )
