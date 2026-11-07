"""Resolve collection inputs, provenance and immutable Native array bundles."""

from __future__ import annotations

import hashlib
import json
from collections.abc import AsyncIterator, Sequence
from dataclasses import dataclass
from time import perf_counter
from typing import cast

import numpy as np
import numpy.typing as npt
from pydantic import JsonValue
from typing_extensions import TypedDict

from nous.application.embedding import EmbeddingOperations
from nous.application.preparation import PreparedText, TextPreparation
from nous.common.errors import NousError
from nous.content.annotations import AnnotationSet, read_annotation
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import (
    Artifact,
    ArtifactRef,
    ContentSelection,
    InputGroup,
    InputPart,
    OutputContent,
    OutputMapping,
    SourceSelection,
)
from nous.content.results import PreparedOutputs, ResultStore
from nous.content.text import TextView
from nous.media.preparation import MediaPreparer
from nous.temporal.values import CognitiveTimes, Instant, parse_explicit_time

from .contracts import (
    Candidate,
    CandidateContent,
    CandidatePath,
    CognitionOwner,
    CorpusView,
    Document,
    DocumentCandidates,
    EvidenceProjection,
    PathStep,
    SnapshotOwner,
    document_order,
)


class ConceptMapping(TypedDict):
    key: str
    label: str
    description: str
    aliases: list[str]


class DocumentMapping(TypedDict):
    artifact: dict[str, str]
    title: str | None
    selector: dict[str, JsonValue]
    roots: list[str]
    derived_from: list[str]
    times: dict[str, object]
    declared_times: dict[str, object] | None
    artifact_times: dict[str, str | None]


class CandidateMapping(TypedDict):
    key: str
    content: dict[str, object]
    origin_contents: list[dict[str, object]]
    retrieval_content: dict[str, object]
    content_document: str
    document: str
    group_key: str
    roots: list[str]
    concepts: list[str]
    paths: list[dict[str, object]]
    metadata: dict[str, JsonValue]
    annotation: dict[str, str] | None


class AssociationMapping(TypedDict):
    key: str
    source: str
    target: str
    relation: str
    directed: bool
    strength: float
    confidence: float
    polarity: str
    support_group: str
    roots: list[str]


class SnapshotMapping(TypedDict):
    owner: dict[str, JsonValue]
    name: str
    metadata: dict[str, JsonValue]
    root_order: list[str]
    documents: dict[str, DocumentMapping]
    roots: dict[str, list[dict[str, str]]]
    concepts: list[ConceptMapping]
    candidates: list[CandidateMapping]
    associations: list[AssociationMapping]
    support_groups: list[dict[str, object]]


class SnapshotRow(TypedDict):
    id: str
    identifier: str
    collection_id: str
    manifest_artifact_id: str | None
    covered_generation: int | None
    profile: str | None
    space_id: str | None
    space_definition: dict[str, object] | None
    identity: dict[str, JsonValue]
    index_location: str
    mapping: SnapshotMapping
    owner: dict[str, JsonValue]
    metadata: dict[str, JsonValue]
    build_key: str | None
    build_signature: str | None
    component_signature: str
    graph_signature: str
    build_summary: dict[str, JsonValue]
    created_at: str


@dataclass(frozen=True)
class FrozenInputs:
    preparation_stats: dict[str, int | float]
    candidate_metadata: dict[str, object]
    candidate_texts: tuple[str, ...]
    candidate_vectors: npt.NDArray[np.float32] | None
    concept_metadata: dict[str, object]
    concept_vectors: npt.NDArray[np.float32] | None
    facts_metadata: dict[str, object]
    sequence_roots: npt.NDArray[np.uint64]
    sequence_indptr: npt.NDArray[np.uint64]
    sequence_indices: npt.NDArray[np.uint64]
    sequence_positions: npt.NDArray[np.int64]
    sequence_ordered: npt.NDArray[np.bool_]
    membership_indptr: npt.NDArray[np.uint64]
    membership_indices: npt.NDArray[np.uint64]
    path_indptr: npt.NDArray[np.uint64]
    path_step_indptr: npt.NDArray[np.uint64]
    path_indices: npt.NDArray[np.uint64]
    path_positions: npt.NDArray[np.int64]
    candidate_root_indptr: npt.NDArray[np.uint64]
    candidate_root_indices: npt.NDArray[np.uint64]
    association_metadata: dict[str, object]
    association_sources: npt.NDArray[np.uint64]
    association_targets: npt.NDArray[np.uint64]
    association_directed: npt.NDArray[np.bool_]
    association_strengths: npt.NDArray[np.float64]
    association_confidences: npt.NDArray[np.float64]
    association_polarities: npt.NDArray[np.int8]
    association_edge_groups: npt.NDArray[np.uint64]
    group_root_indptr: npt.NDArray[np.uint64]
    group_root_indices: npt.NDArray[np.uint64]
    vector_mappings: tuple[dict[str, object], ...]
    candidate_data: tuple[dict[str, JsonValue], ...]
    mapping: SnapshotMapping


def _csr(rows: list[list[int]]) -> tuple[npt.NDArray[np.uint64], npt.NDArray[np.uint64]]:
    offsets = [0]
    values: list[int] = []
    for row in rows:
        values.extend(row)
        offsets.append(len(values))
    return np.asarray(offsets, dtype=np.uint64), np.asarray(values, dtype=np.uint64)


def projection_inputs(view: CorpusView) -> dict[str, dict[str, object]]:
    """Separate exact per-document inputs from the small snapshot-wide view."""
    documents = {document.key: document for document in view.documents}
    declarations_by_document: dict[str, list[Candidate | DocumentCandidates]] = {}
    for declaration in view.candidates:
        declarations_by_document.setdefault(declaration.document, []).append(declaration)
    result: dict[str, dict[str, object]] = {}
    for document in view.documents:
        declarations = declarations_by_document.get(document.key, [])
        sources = {item.content.document for item in declarations} | {document.key}
        roots = {root for key in sources for root in view.evidence.documents[key]}
        payload = {
            "owner": view.owner.model_dump(mode="json"),
            "documents": [documents[key].model_dump(mode="json") for key in sorted(sources)],
            "declarations": [item.model_dump(mode="json") for item in declarations],
            "sequences": [
                item.model_dump(mode="json")
                for item in view.source_sequences
                if item.document in sources
            ],
            "document_roots": {key: list(view.evidence.documents[key]) for key in sorted(sources)},
            "roots": {
                key: [ref.model_dump(mode="json") for ref in view.evidence.roots[key]]
                for key in sorted(roots)
            },
        }
        result[document.key] = cast(dict[str, object], payload)
    return result


def _document_times(
    document_times: CognitiveTimes | None, artifact: Artifact, owner: SnapshotOwner
) -> dict[str, object]:
    declared = document_times.model_dump(mode="json") if document_times is not None else {}
    # A cognitive revision records its own transaction time. A manually supplied
    # manifest can declare other axes, but cannot replace the Artifact's recorded time.
    if not isinstance(owner, CognitionOwner) or declared.get("recorded") is None:
        recorded = parse_explicit_time(artifact.recorded_at.isoformat())
        assert isinstance(recorded, Instant)
        declared["recorded"] = recorded.at.model_dump(mode="json")
    if declared.get("formed") is None and artifact.formed_at is not None:
        formed = parse_explicit_time(artifact.formed_at.isoformat())
        assert isinstance(formed, Instant)
        declared["formed"] = formed.model_dump(mode="json")
    return declared


async def content_evidence(
    documents: Sequence[Document], accepted: dict[str, ArtifactRef], results: ResultStore
) -> EvidenceProjection:
    """Resolve and validate an imported collection's declared parents against adopted content."""
    document_values = documents
    documents_by_key = {document.key: document for document in document_values}
    provenance = await results.provenance(tuple(accepted.values()))
    root_artifacts: dict[str, list[ArtifactRef]] = {}
    document_roots: dict[str, tuple[str, ...]] = {}

    for key in document_order(document_values):
        document = documents_by_key[key]
        summary = provenance[accepted[key].id]
        if not summary.complete:
            raise NousError(
                "wave.provenance_incomplete",
                "Cannot freeze incomplete source lineage",
                details={"document": key},
            )
        if document.derived_from:
            keys = tuple(
                dict.fromkeys(
                    root for parent in document.derived_from for root in document_roots[parent]
                )
            )
            actual = {ref.id for ref in summary.support_roots}
            if actual and actual != {accepted[key].id}:
                declared = {ref.id for root in keys for ref in root_artifacts[root]}
                if actual != declared:
                    raise NousError(
                        "wave.provenance_conflict",
                        "Declared parents conflict with the adopted result lineage",
                        details={"document": key},
                    )
        else:
            ref = accepted[key]
            summary = provenance[ref.id]
            if not summary.complete:
                raise NousError(
                    "wave.provenance_incomplete",
                    "Cannot freeze incomplete source lineage",
                    details={"artifact": ref.identifier},
                )
            origins = summary.support_roots
            if document.evidence_group and len(origins) != 1:
                raise NousError(
                    "wave.invalid_evidence_group",
                    "A multiple-origin document cannot be assigned to one evidence group",
                    details={"document": key},
                )
            keys_list: list[str] = []
            for origin in origins:
                root = (
                    f"group:{document.evidence_group}"
                    if document.evidence_group is not None
                    else f"artifact:{origin.id}"
                )
                refs = root_artifacts.setdefault(root, [])
                if origin not in refs:
                    refs.append(origin)
                keys_list.append(root)
            keys = tuple(dict.fromkeys(keys_list))
        document_roots[key] = keys
    source_documents = tuple(
        key
        for key, document in documents_by_key.items()
        if not document.derived_from
        and len(provenance[accepted[key].id].terminal_roots) == 1
        and provenance[accepted[key].id].terminal_roots[0].id == accepted[key].id
    )
    return EvidenceProjection(
        roots={key: tuple(refs) for key, refs in root_artifacts.items()},
        documents=document_roots,
        source_documents=source_documents,
    )


async def _prepare_candidates(
    manifest: CorpusView,
    documents: dict[str, Document],
    accepted: dict[str, ArtifactRef],
    preparation: TextPreparation,
    cached: dict[str, list[dict[str, object]]],
) -> AsyncIterator[tuple[Candidate | None, PreparedText | None, dict[str, object] | None]]:
    """Normalize exact inputs and document layouts through one content preparation session."""
    await preparation.prepare_many(
        tuple(
            ContentSelection(
                artifact=accepted[documents[declaration.content.document].key],
                selection=documents[declaration.content.document].input.selector.restrict(
                    declaration.content.selector
                ),
            )
            for declaration in manifest.candidates
            if declaration.document not in cached
        )
    )
    for declaration in manifest.candidates:
        if declaration.document in cached:
            for item in cached[declaration.document]:
                key = cast(str, cast(dict[str, object], item["mapping"])["key"])
                if (
                    isinstance(declaration, Candidate)
                    and key == declaration.key
                    or isinstance(declaration, DocumentCandidates)
                    and (
                        key == f"{declaration.group_key}:whole"
                        or key.startswith(f"{declaration.group_key}:part:")
                    )
                ):
                    yield None, None, item
            continue
        document = documents[declaration.content.document]
        origin = ContentSelection(
            artifact=accepted[document.key],
            selection=document.input.selector.restrict(declaration.content.selector),
        )
        prepared = await preparation.prepare(origin)
        if isinstance(declaration, Candidate):
            yield declaration, prepared, None
            continue
        assert isinstance(declaration, DocumentCandidates)

        def exact(
            key: str, selector: SourceSelection, declaration: DocumentCandidates = declaration
        ) -> Candidate:
            return Candidate(
                key=key,
                document=declaration.document,
                group_key=declaration.group_key,
                content=CandidateContent(document=declaration.document, selector=selector),
                concepts=declaration.concepts,
                occurrences=declaration.occurrences,
                paths=declaration.paths,
                annotation_ref=declaration.annotation_ref,
                annotation_concepts=declaration.annotation_concepts,
                metadata=declaration.metadata,
            )

        if declaration.whole:
            yield exact(f"{declaration.group_key}:whole", SourceSelection()), prepared, None
        if declaration.local_layout is not None:
            chunks = await preparation.artifacts.content.execution.blocking(
                prepared.view.complete_chunks, declaration.local_layout.target_bytes
            )
            for index, chunk in enumerate(chunks, 1):
                if declaration.whole and chunk.text == prepared.view.text:
                    continue
                selector = chunk.selection.selection
                yield (
                    exact(f"{declaration.group_key}:part:{index}", selector),
                    await preparation.artifacts.content.execution.blocking(
                        prepared.slice, selector
                    ),
                    None,
                )


async def _covers(
    prepared: PreparedText, selections: Sequence[SourceSelection], preparation: TextPreparation
) -> bool:
    low = prepared.view.byte_offset
    high = low + len(prepared.view.text.encode("utf-8"))
    normalized = await preparation.artifacts.normalize_selections(
        tuple(
            ContentSelection(artifact=prepared.view.body.artifact, selection=part)
            for part in selections
        )
    )
    for value in normalized:
        start, end = value.selection.start_byte, value.selection.end_byte
        assert start is not None and end is not None
        if start < low or end > high:
            return False
    return True


async def _titled_semantics(
    preparation: TextPreparation,
    contents: Sequence[ContentSelection],
    texts: Sequence[str],
    titles: Sequence[str],
) -> tuple[ContentSelection, ...]:
    """Adopt title-plus-body inputs, mapping only their body back to source text."""
    requests: dict[str, tuple[ContentSelection, str, str]] = {}
    order: list[str | None] = []
    for content, text, title in zip(contents, texts, titles, strict=True):
        if not title:
            order.append(None)
            continue
        key = hashlib.sha256(
            json.dumps(
                {"content": content.model_dump(mode="json"), "title": title},
                ensure_ascii=False,
                sort_keys=True,
            ).encode("utf-8")
        ).hexdigest()
        requests[key] = (content, text, title)
        order.append(key)
    results = preparation.preparer.results
    async with results.database.read() as tx:
        adopted = await results.cached_outputs_in(
            tx, "wave.titled_semantic_text", tuple(requests), output="text"
        )
    pending = [key for key in requests if key not in adopted]
    for start in range(0, len(pending), 100):
        keys = pending[start : start + 100]
        sources = await preparation.artifacts.normalize_selections(
            [requests[key][0] for key in keys]
        )
        artifacts = await preparation.artifacts.get_many([source.artifact for source in sources])
        outputs: list[PreparedOutputs] = []
        for key in keys:
            _, text, title = requests[key]
            outputs.append(
                await results.prepare_outputs(
                    {
                        "text": OutputContent(
                            data=f"{title}\n\n{text}".encode(),
                            media_type="text/plain",
                            display_name="retrieval-text.txt",
                        )
                    }
                )
            )
        async with results.database.write() as tx:
            adopted.update(
                await results.cached_outputs_in(
                    tx, "wave.titled_semantic_text", keys, output="text"
                )
            )
            for key, source, artifact, output in zip(
                keys, sources, artifacts, outputs, strict=True
            ):
                if key in adopted:
                    continue
                _, text, title = requests[key]
                offset = len(f"{title}\n\n".encode())
                result = await results.adopt_in(
                    tx,
                    operation="wave.titled_semantic_text",
                    producer={"kind": "deterministic"},
                    inputs=(
                        InputGroup(
                            key="body",
                            slot_key="body",
                            name="Body",
                            parts=(
                                InputPart(
                                    content=source,
                                    representation="semantic_text",
                                    media_type=artifact.media_type,
                                ),
                            ),
                        ),
                    ),
                    prepared_outputs=output,
                    parameters={"cache_key": key, "title": title, "title_field": "title"},
                    mappings=(
                        OutputMapping(
                            output_name="text",
                            relation="identity",
                            origins=(source,),
                            selection=SourceSelection(
                                start_byte=offset, end_byte=offset + len(text.encode("utf-8"))
                            ),
                        ),
                    ),
                )
                adopted[key] = (
                    result.ref,
                    next(value.artifact for value in result.outputs if value.name == "text"),
                )
    return tuple(
        content if key is None else ContentSelection(artifact=adopted[key][1])
        for content, key in zip(contents, order, strict=True)
    )


async def freeze_inputs(
    manifest: CorpusView,
    space_id: str | None,
    artifacts: ArtifactStore,
    embedding: EmbeddingOperations,
    preparer: MediaPreparer,
    cached_units: dict[str, list[dict[str, object]]],
) -> FrozenInputs:
    """Admit documents once and preserve independent roots through all derived candidates."""
    prepared_at = perf_counter()
    documents = {item.key: item for item in manifest.documents}
    accepted: dict[str, ArtifactRef] = {}
    accepted_artifacts: dict[str, Artifact] = {}
    document_refs = [document.input.artifact for document in manifest.documents]
    assert all(ref is not None for ref in document_refs)
    root_artifacts = {key: list(refs) for key, refs in manifest.evidence.roots.items()}
    document_roots = manifest.evidence.documents
    root_refs = tuple(dict.fromkeys(ref for refs in root_artifacts.values() for ref in refs))
    all_artifacts = await artifacts.get_many([*cast(list[str], document_refs), *root_refs])
    for document, artifact in zip(
        manifest.documents, all_artifacts[: len(manifest.documents)], strict=True
    ):
        accepted[document.key] = artifact.ref
        accepted_artifacts[document.key] = artifact

    root_keys = list(root_artifacts)
    root_rows = {key: index for index, key in enumerate(root_keys)}
    concept_rows = {concept.key: index for index, concept in enumerate(manifest.concepts)}
    document_sequences: dict[str, list[tuple[tuple[str, ...], bool]]] = {}
    source_rows: list[list[int]] = []
    source_roots: list[int] = []
    source_ordered: list[bool] = []
    sequence_keys: list[str] = []
    for index, sequence in enumerate(manifest.source_sequences):
        document = documents[sequence.document]
        if sequence.document not in manifest.evidence.source_documents:
            continue
        document_sequences.setdefault(sequence.document, []).append(
            (sequence.concepts, sequence.ordered)
        )
        source_rows.append([concept_rows[key] for key in sequence.concepts])
        source_roots.append(root_rows[document_roots[sequence.document][0]])
        source_ordered.append(sequence.ordered)
        sequence_keys.append(f"{sequence.document}:{index}")

    candidate_texts: list[str] = []
    candidate_titles: list[str] = []
    indexed_titles: list[str] = []
    candidate_contents: list[ContentSelection] = []
    embedding_contents: list[ContentSelection] = []
    prepared_candidates: list[PreparedText] = []
    fresh_indices: list[int] = []
    semantic_slots: list[ContentSelection | None] = []
    candidate_position_groups: list[list[list[int]]] = []
    candidate_memberships: list[list[int]] = []
    candidate_paths: list[list[int]] = []
    candidate_path_counts: list[int] = []
    candidate_positions: list[list[int]] = []
    candidate_roots: list[list[int]] = []
    candidate_mapping: list[CandidateMapping] = []
    preparation = TextPreparation(artifacts, preparer, known_artifacts=all_artifacts)
    await preparation.prime()
    annotations: dict[str, tuple[Artifact, AnnotationSet, TextView]] = {}
    async for candidate, prepared, cached in _prepare_candidates(
        manifest, documents, accepted, preparation, cached_units
    ):
        if cached is not None:
            mapping = cast(CandidateMapping, cached["mapping"])
            positions = cast(list[list[int]], cached["path_positions"])
            candidate_mapping.append(mapping)
            candidate_contents.append(ContentSelection.model_validate(mapping["content"]))
            semantic_slots.append(ContentSelection.model_validate(mapping["retrieval_content"]))
            candidate_texts.append(cast(str, cached["text"]))
            candidate_titles.append(documents[mapping["content_document"]].title or "")
            indexed_titles.append(cast(str, cached["indexed_title"]))
            candidate_memberships.append([concept_rows[key] for key in mapping["concepts"]])
            for path in mapping["paths"]:
                candidate_paths.append(
                    [
                        concept_rows[cast(str, step["concept"])]
                        for step in cast(list[dict[str, object]], path["steps"])
                    ]
                )
            candidate_positions.extend(positions)
            candidate_position_groups.append(positions)
            candidate_path_counts.append(len(positions))
            candidate_roots.append([root_rows[key] for key in mapping["roots"]])
            continue
        assert candidate is not None and prepared is not None
        document = documents[candidate.content.document]
        origin_contents = prepared.origin_contents
        selection = prepared.view.body
        if not prepared.text.strip():
            continue
        candidate_contents.append(selection)
        fresh_indices.append(len(candidate_mapping))
        semantic_slots.append(None)
        prepared_candidates.append(prepared)
        retrieval_content = selection
        candidate_texts.append(prepared.text)
        candidate_titles.append(document.title or "")
        indexed_titles.append((document.title or "") if candidate.content.selector.is_whole else "")
        inherited = document_sequences.get(candidate.content.document, [])
        can_inherit = (
            candidate.content.selector.is_whole
            and document.input.selector.is_whole
            and len(document_roots[candidate.content.document]) == 1
            and len(inherited) == 1
        )
        paths = list(candidate.paths or ())
        annotation_artifact = None
        annotation_memberships: tuple[str, ...] | None = None
        if candidate.occurrences:
            covered_concepts = []
            for occurrence in candidate.occurrences:
                if await _covers(prepared, occurrence.selections, preparation):
                    covered_concepts.append(occurrence.concept)
            annotation_memberships = tuple(dict.fromkeys(covered_concepts))
        if candidate.annotation_ref is not None:
            cached_annotation = annotations.get(candidate.annotation_ref)
            if cached_annotation is None:
                annotation_artifact = await artifacts.get(candidate.annotation_ref)
                annotation = await read_annotation(artifacts, annotation_artifact.ref)
                annotated = await preparation.prepare(
                    ContentSelection(artifact=annotation.body_ref)
                )
                cached_annotation = annotation_artifact, annotation, annotated.view
                annotations[candidate.annotation_ref] = cached_annotation
            annotation_artifact, annotation, view = cached_annotation
            if annotation.body_ref != selection.artifact:
                raise NousError(
                    "wave.annotation_mismatch",
                    "Annotation belongs to another body",
                    details={"candidate": candidate.key},
                )
            blocks = {block.key: block for block in view.blocks}
            occurrences: dict[str, PathStep] = {}
            covered_concepts: list[str] = []
            low = prepared.view.byte_offset
            high = low + len(prepared.view.text.encode("utf-8"))
            for occurrence in annotation.occurrences:
                mapped = candidate.annotation_concepts.get(occurrence.concept, occurrence.concept)
                if mapped not in concept_rows:
                    raise NousError(
                        "wave.concept_not_found",
                        "Annotation concept needs an explicit manifest mapping",
                        details={"concept": occurrence.concept},
                    )
                covered = [blocks[key].selection.selection for key in occurrence.blocks]
                occurrences[occurrence.key] = PathStep(concept=mapped, selections=tuple(covered))
                if all(
                    part.start_byte is not None
                    and part.end_byte is not None
                    and low <= part.start_byte
                    and part.end_byte <= high
                    for part in covered
                ):
                    covered_concepts.append(mapped)
            # Membership is a fact about covered occurrences, independent of declared paths.
            annotation_memberships = tuple(dict.fromkeys(covered_concepts))
            for path in annotation.paths:
                paths.append(
                    CandidatePath(
                        key=path.key,
                        label=path.label,
                        steps=tuple(occurrences[key] for key in path.occurrences),
                    )
                )
        if (
            candidate.paths is None
            and candidate.annotation_ref is None
            and can_inherit
            and inherited[0][1]
            and inherited[0][0]
        ):
            paths = [
                CandidatePath(
                    key="source", steps=tuple(PathStep(concept=key) for key in inherited[0][0])
                )
            ]
        # Clipping splits at each excluded occurrence; no path joins across a gap.
        selected_paths: list[tuple[CandidatePath, list[tuple[int, PathStep]]]] = []
        for path in paths:
            current: list[tuple[int, PathStep]] = []
            for position, step in enumerate(path.steps, 1):
                inside = await _covers(prepared, step.selections, preparation)
                if inside:
                    current.append((position, step))
                elif current:
                    selected_paths.append((path, current))
                    current = []
            if current:
                selected_paths.append((path, current))
        if len(selected_paths) > 16:
            raise NousError(
                "wave.path_limit",
                "A candidate may contain at most 16 local paths",
                details={"candidate": candidate.key},
            )
        memberships = candidate.concepts
        if memberships is None:
            if annotation_memberships is not None:
                memberships = annotation_memberships
            else:
                memberships = tuple(
                    dict.fromkeys(step.concept for _, steps in selected_paths for _, step in steps)
                )
                if not memberships and can_inherit:
                    memberships = tuple(dict.fromkeys(inherited[0][0]))
        required_concepts = {step.concept for _, steps in selected_paths for _, step in steps}
        required_concepts.update(annotation_memberships or ())
        missing = required_concepts - set(memberships)
        if missing:
            raise NousError(
                "wave.invalid_membership",
                "Covered annotation and path concepts must belong to candidate memberships",
                details={"candidate": candidate.key, "concepts": sorted(missing)},
            )
        path_mapping: list[dict[str, object]] = []
        for path, steps in selected_paths:
            candidate_paths.append([concept_rows[step.concept] for _, step in steps])
            candidate_positions.append([position for position, _ in steps])
            path_mapping.append(
                {
                    "key": path.key,
                    "label": path.label,
                    "steps": [step.model_dump(mode="json") for _, step in steps],
                }
            )
        candidate_path_counts.append(len(selected_paths))
        candidate_position_groups.append(
            [[position for position, _ in steps] for _, steps in selected_paths]
        )
        candidate_memberships.append([concept_rows[key] for key in dict.fromkeys(memberships)])
        roots_for_candidate = document_roots[candidate.content.document]
        candidate_roots.append([root_rows[key] for key in roots_for_candidate])
        candidate_mapping.append(
            {
                "key": candidate.key,
                "content": selection.model_dump(mode="json"),
                "origin_contents": [part.model_dump(mode="json") for part in origin_contents],
                "retrieval_content": retrieval_content.model_dump(mode="json"),
                "content_document": candidate.content.document,
                "document": candidate.document,
                "group_key": candidate.group_key or candidate.document,
                "roots": list(roots_for_candidate),
                "concepts": list(dict.fromkeys(memberships)),
                "paths": path_mapping,
                "metadata": candidate.metadata,
                "annotation": annotation_artifact.ref.model_dump(mode="json")
                if annotation_artifact is not None
                else None,
            }
        )

    fresh_semantics = await preparation.semantic_contents(prepared_candidates)
    for index, content in zip(fresh_indices, fresh_semantics, strict=True):
        candidate_mapping[index]["retrieval_content"] = content.model_dump(mode="json")
        semantic_slots[index] = content
    assert all(content is not None for content in semantic_slots)
    semantic_contents = cast(list[ContentSelection], semantic_slots)
    if space_id is not None:
        embedding_contents.extend(
            await _titled_semantics(
                preparation, semantic_contents, candidate_texts, candidate_titles
            )
        )

    support_groups: list[dict[str, object]] = []
    group_mapping: list[dict[str, object]] = []
    support_group_rows = {group.key: index for index, group in enumerate(manifest.support_groups)}
    group_roots: dict[str, tuple[str, ...]] = {}
    for group in manifest.support_groups:
        evidence = tuple(
            dict.fromkeys(
                root for document in group.required_sources for root in document_roots[document]
            )
        )
        group_roots[group.key] = evidence
        basis = [
            ContentSelection(
                artifact=accepted[ref.document],
                selection=documents[ref.document].input.selector.restrict(ref.selector),
            ).model_dump(mode="json")
            for ref in group.basis_refs
        ]
        origin = (
            ContentSelection(
                artifact=accepted[group.origin_ref.document],
                selection=documents[group.origin_ref.document].input.selector.restrict(
                    group.origin_ref.selector
                ),
            ).model_dump(mode="json")
            if group.origin_ref
            else None
        )
        group_mapping.append(
            {
                "key": group.key,
                "roots": list(evidence),
                "kind": group.kind,
                "basis_refs": basis,
                "origin_ref": origin,
            }
        )
        support_groups.append(
            {
                "key": group.key,
                "kind": group.kind,
            }
        )
    association_mapping: list[AssociationMapping] = []
    for association in manifest.associations:
        evidence = group_roots[association.support_group]
        association_mapping.append(
            cast(
                AssociationMapping, {**association.model_dump(mode="json"), "roots": list(evidence)}
            )
        )

    content_prepare_ms = (perf_counter() - prepared_at) * 1000
    embedding_at = perf_counter()
    embedding_ensure_calls = 0
    candidate_vectors: npt.NDArray[np.float32] | None = None
    concept_vectors: npt.NDArray[np.float32] | None = None
    vector_mappings: list[dict[str, object]] = []
    if space_id is not None:
        if candidate_contents:
            embedding_ensure_calls += 1
            batch = await embedding.ensure(embedding_contents, space_id, "document")
            candidate_vectors = await embedding.read(batch)
            vector_mappings.extend(
                {
                    "kind": "candidate",
                    "key": candidate["key"],
                    "input": source.model_dump(mode="json"),
                    "result_id": row.result_id,
                    "row_index": row.row_index,
                }
                for candidate, source, row in zip(
                    candidate_mapping, batch.inputs, batch.rows, strict=True
                )
            )
        concept_contents: list[ContentSelection] = []
        for concept in manifest.concepts:
            concept_contents.append(
                await preparation.literal(
                    concept.embedding_text(), display_name=f"{concept.key}.txt"
                )
            )
        if concept_contents:
            embedding_ensure_calls += 1
            batch = await embedding.ensure(concept_contents, space_id, "document")
            concept_vectors = await embedding.read(batch)
            vector_mappings.extend(
                {
                    "kind": "concept",
                    "key": concept.key,
                    "input": source.model_dump(mode="json"),
                    "result_id": row.result_id,
                    "row_index": row.row_index,
                }
                for concept, source, row in zip(
                    manifest.concepts, batch.inputs, batch.rows, strict=True
                )
            )

    embedding_ms = (perf_counter() - embedding_at) * 1000
    sequence_indptr, sequence_indices = _csr(source_rows)
    membership_indptr, membership_indices = _csr(candidate_memberships)
    path_step_indptr, path_indices = _csr(candidate_paths)
    path_indptr, _ = _csr([list(range(count)) for count in candidate_path_counts])
    candidate_root_indptr, candidate_root_indices = _csr(candidate_roots)
    group_root_indptr, group_root_indices = _csr(
        [[root_rows[root] for root in group_roots[group.key]] for group in manifest.support_groups]
    )
    group_keys = list(dict.fromkeys(item["group_key"] for item in candidate_mapping))
    candidate_group_rows = {key: row for row, key in enumerate(group_keys)}
    return FrozenInputs(
        preparation_stats={
            **preparation.stats,
            "documents": len(manifest.documents),
            "candidates": len(candidate_mapping),
            "content_prepare_ms": content_prepare_ms,
            "embedding_ms": embedding_ms,
            "embedding_ensure_calls": embedding_ensure_calls,
            "reused_projection_units": len(cached_units),
        },
        candidate_metadata={
            "keys": [item["key"] for item in candidate_mapping],
            "documents": [item["document"] for item in candidate_mapping],
            "titles": indexed_titles,
            "group_keys": group_keys,
            "group_rows": [candidate_group_rows[item["group_key"]] for item in candidate_mapping],
        },
        candidate_texts=tuple(candidate_texts),
        candidate_vectors=candidate_vectors,
        concept_metadata={
            "keys": [item.key for item in manifest.concepts],
            "labels": [item.label for item in manifest.concepts],
            "aliases": [list(item.aliases) for item in manifest.concepts],
        },
        concept_vectors=concept_vectors,
        facts_metadata={
            "root_keys": root_keys,
            "root_checksums": [
                hashlib.sha256(key.encode("utf-8")).hexdigest() for key in root_keys
            ],
            "sequence_keys": sequence_keys,
        },
        sequence_roots=np.asarray(source_roots, dtype=np.uint64),
        sequence_indptr=sequence_indptr,
        sequence_indices=sequence_indices,
        sequence_positions=np.asarray(
            [position for row in source_rows for position in range(1, len(row) + 1)], dtype=np.int64
        ),
        sequence_ordered=np.asarray(source_ordered, dtype=np.bool_),
        membership_indptr=membership_indptr,
        membership_indices=membership_indices,
        path_indptr=path_indptr,
        path_step_indptr=path_step_indptr,
        path_indices=path_indices,
        path_positions=np.asarray(
            [position for row in candidate_positions for position in row], dtype=np.int64
        ),
        candidate_root_indptr=candidate_root_indptr,
        candidate_root_indices=candidate_root_indices,
        association_metadata={
            "keys": [item.key for item in manifest.associations],
            "relations": [item.relation for item in manifest.associations],
            "support_groups": support_groups,
        },
        association_sources=np.asarray(
            [concept_rows[item.source] for item in manifest.associations], dtype=np.uint64
        ),
        association_targets=np.asarray(
            [concept_rows[item.target] for item in manifest.associations], dtype=np.uint64
        ),
        association_directed=np.asarray(
            [item.directed for item in manifest.associations], dtype=np.bool_
        ),
        association_strengths=np.asarray(
            [item.strength for item in manifest.associations], dtype=np.float64
        ),
        association_confidences=np.asarray(
            [item.confidence for item in manifest.associations], dtype=np.float64
        ),
        association_polarities=np.asarray(
            [1 if item.polarity == "positive" else -1 for item in manifest.associations],
            dtype=np.int8,
        ),
        association_edge_groups=np.asarray(
            [support_group_rows[item.support_group] for item in manifest.associations],
            dtype=np.uint64,
        ),
        group_root_indptr=group_root_indptr,
        group_root_indices=group_root_indices,
        vector_mappings=tuple(vector_mappings),
        candidate_data=tuple(
            cast(
                dict[str, JsonValue],
                {"text": text, "path_positions": positions, "indexed_title": title},
            )
            for text, positions, title in zip(
                candidate_texts, candidate_position_groups, indexed_titles, strict=True
            )
        ),
        mapping={
            "name": manifest.name,
            "owner": cast(dict[str, JsonValue], manifest.owner.model_dump(mode="json")),
            "metadata": manifest.metadata,
            "root_order": root_keys,
            "documents": {
                item.key: {
                    "artifact": accepted[item.key].model_dump(mode="json"),
                    "title": item.title,
                    "selector": item.input.selector.model_dump(mode="json"),
                    "roots": list(document_roots[item.key]),
                    "derived_from": list(item.derived_from),
                    "times": _document_times(
                        item.times, accepted_artifacts[item.key], manifest.owner
                    ),
                    "declared_times": item.times.model_dump(mode="json") if item.times else None,
                    "artifact_times": {
                        "received_at": received.isoformat()
                        if (received := accepted_artifacts[item.key].received_at) is not None
                        else None,
                        "formed_at": formed.isoformat()
                        if (formed := accepted_artifacts[item.key].formed_at) is not None
                        else None,
                        "recorded_at": accepted_artifacts[item.key].recorded_at.isoformat(),
                    },
                }
                for item in manifest.documents
            },
            "roots": {
                key: [ref.model_dump(mode="json") for ref in refs]
                for key, refs in root_artifacts.items()
            },
            "concepts": [
                cast(ConceptMapping, item.model_dump(mode="json")) for item in manifest.concepts
            ],
            "candidates": candidate_mapping,
            "associations": association_mapping,
            "support_groups": group_mapping,
        },
    )
