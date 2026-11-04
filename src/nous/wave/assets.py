"""Resolve collection inputs, provenance and immutable Native array bundles."""

from __future__ import annotations

import hashlib
from collections.abc import Sequence
from dataclasses import dataclass
from typing import cast

import numpy as np
import numpy.typing as npt
from pydantic import JsonValue
from typing_extensions import TypedDict

from nous.application.embedding import EmbeddingOperations
from nous.common.errors import NousError
from nous.content.annotations import read_annotation
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import (
    Artifact,
    ArtifactRef,
    ContentSelection,
    SourceSelection,
)
from nous.content.projections import semantic_text
from nous.content.results import ResultStore
from nous.media.preparation import MediaPreparer
from nous.temporal.values import CognitiveTimes, Instant, parse_explicit_time

from .contracts import CandidatePath, CorpusView, Document, EvidenceProjection, PathStep


class ConceptMapping(TypedDict):
    key: str
    label: str
    description: str
    aliases: list[str]


class DocumentMapping(TypedDict):
    artifact: dict[str, str]
    selector: dict[str, JsonValue]
    roots: list[str]
    derived_from: list[str]
    times: dict[str, object]
    declared_times: dict[str, object] | None
    artifact_times: dict[str, str | None]


class CandidateMapping(TypedDict):
    key: str
    content: dict[str, object]
    origin_content: dict[str, object]
    retrieval_content: dict[str, object]
    content_document: str
    document: str
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
    address: str
    collection_id: str
    manifest_artifact_id: str | None
    covered_generation: int | None
    profile: str | None
    space_id: str | None
    space_definition: dict[str, object] | None
    identity: dict[str, JsonValue]
    index_location: str
    mapping: SnapshotMapping
    created_at: str


@dataclass(frozen=True)
class FrozenInputs:
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
    mapping: SnapshotMapping


def _csr(rows: list[list[int]]) -> tuple[npt.NDArray[np.uint64], npt.NDArray[np.uint64]]:
    offsets = [0]
    values: list[int] = []
    for row in rows:
        values.extend(row)
        offsets.append(len(values))
    return np.asarray(offsets, dtype=np.uint64), np.asarray(values, dtype=np.uint64)


def _document_times(document_times: CognitiveTimes | None, artifact: Artifact) -> dict[str, object]:
    declared = document_times.model_dump(mode="json") if document_times is not None else {}
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

    def roots(key: str) -> tuple[str, ...]:
        if key in document_roots:
            return document_roots[key]
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
                dict.fromkeys(root for parent in document.derived_from for root in roots(parent))
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
                    details={"artifact": ref.address},
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
        return keys

    for key in documents_by_key:
        roots(key)
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


async def freeze_inputs(
    manifest: CorpusView,
    space_id: str | None,
    artifacts: ArtifactStore,
    embedding: EmbeddingOperations,
    preparer: MediaPreparer,
) -> FrozenInputs:
    """Admit documents once and preserve independent roots through all derived candidates."""
    documents = {item.key: item for item in manifest.documents}
    accepted: dict[str, ArtifactRef] = {}
    accepted_artifacts: dict[str, Artifact] = {}
    for document in manifest.documents:
        source = document.input
        assert source.artifact is not None
        artifact = await artifacts.get(source.artifact)
        accepted[document.key] = artifact.ref
        accepted_artifacts[document.key] = artifact

    root_artifacts = {key: list(refs) for key, refs in manifest.evidence.roots.items()}
    document_roots = manifest.evidence.documents
    for refs in root_artifacts.values():
        for ref in refs:
            await artifacts.get(ref)
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
    candidate_contents: list[ContentSelection] = []
    embedding_contents: list[ContentSelection] = []
    candidate_memberships: list[list[int]] = []
    candidate_paths: list[list[int]] = []
    candidate_path_counts: list[int] = []
    candidate_positions: list[list[int]] = []
    candidate_roots: list[list[int]] = []
    candidate_mapping: list[CandidateMapping] = []
    for candidate in manifest.candidates:
        document = documents[candidate.content.document]
        selection = ContentSelection(
            artifact=accepted[candidate.content.document],
            selection=document.input.selector.restrict(candidate.content.selector),
        )
        origin_content = selection
        representation = await preparer.text_view(
            origin_content, settings=preparer.execution.snapshot.typed_values().media
        )
        selection = representation.view.body
        projected = await semantic_text(selection, preparer.results)
        if not projected.text.strip():
            continue
        candidate_contents.append(selection)
        embedding_contents.append(projected.content)
        candidate_texts.append(projected.text)
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
        if candidate.annotation_ref is not None:
            annotation_artifact = await artifacts.get(candidate.annotation_ref)
            annotation = await read_annotation(artifacts, annotation_artifact.ref)
            if annotation.body_ref != selection.artifact:
                raise NousError(
                    "wave.annotation_mismatch",
                    "Annotation belongs to another body",
                    details={"candidate": candidate.key},
                )
            view = await artifacts.text_view(annotation.body_ref)
            blocks = {block.key: block for block in view.blocks}
            occurrences: dict[str, PathStep] = {}
            covered_concepts: list[str] = []
            low = representation.view.byte_offset
            high = low + len(representation.view.text.encode("utf-8"))
            for occurrence in annotation.occurrences:
                mapped = candidate.annotation_concepts.get(occurrence.concept, occurrence.concept)
                if mapped not in concept_rows:
                    raise NousError(
                        "wave.concept_not_found",
                        "Annotation concept needs an explicit manifest mapping",
                        details={"concept": occurrence.concept},
                    )
                covered = [blocks[key].selection.selection for key in occurrence.blocks]
                start = min(part.start_byte for part in covered if part.start_byte is not None)
                end = max(part.end_byte for part in covered if part.end_byte is not None)
                occurrences[occurrence.key] = PathStep(
                    concept=mapped,
                    selection=SourceSelection(start_byte=start, end_byte=end),
                )
                if low <= start and end <= high:
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
        path_view = representation.view if paths else None
        # Clipping splits paths at excluded occurrences; it never joins across a gap.
        selected_paths: list[tuple[CandidatePath, list[tuple[int, PathStep]]]] = []
        for path in paths:
            current: list[tuple[int, PathStep]] = []
            for position, step in enumerate(path.steps, 1):
                part = step.selection
                scope = selection.selection
                inside = True
                if path_view is not None:
                    low = path_view.byte_offset
                    high = low + len(path_view.text.encode("utf-8"))
                    if part.start_line is not None or part.end_line is not None:
                        located = await artifacts.text_view(
                            ContentSelection(artifact=selection.artifact, selection=part)
                        )
                        sub_low = located.byte_offset
                        sub_high = sub_low + len(located.text.encode("utf-8"))
                    elif part.start_byte is not None or part.end_byte is not None:
                        sub_low = part.start_byte or 0
                        sub_high = part.end_byte
                    else:
                        if not scope.is_whole:
                            raise NousError(
                                "wave.path_selection_required",
                                "Local paths need exact body positions",
                                details={"candidate": candidate.key, "path": path.key},
                            )
                        sub_low, sub_high = low, high
                    inside = sub_high is not None and sub_low >= low and sub_high <= high
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
        candidate_memberships.append([concept_rows[key] for key in dict.fromkeys(memberships)])
        roots_for_candidate = document_roots[candidate.content.document]
        candidate_roots.append([root_rows[key] for key in roots_for_candidate])
        candidate_mapping.append(
            {
                "key": candidate.key,
                "content": selection.model_dump(mode="json"),
                "origin_content": origin_content.model_dump(mode="json"),
                "retrieval_content": projected.content.model_dump(mode="json"),
                "content_document": candidate.content.document,
                "document": candidate.document,
                "roots": list(roots_for_candidate),
                "concepts": list(dict.fromkeys(memberships)),
                "paths": path_mapping,
                "metadata": candidate.metadata,
                "annotation": annotation_artifact.ref.model_dump(mode="json")
                if annotation_artifact is not None
                else None,
            }
        )

    support_groups: list[dict[str, object]] = []
    group_mapping: list[dict[str, object]] = []
    group_rows = {group.key: index for index, group in enumerate(manifest.support_groups)}
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

    candidate_vectors: npt.NDArray[np.float32] | None = None
    concept_vectors: npt.NDArray[np.float32] | None = None
    vector_mappings: list[dict[str, object]] = []
    if space_id is not None:
        if candidate_contents:
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
            artifact = await artifacts.ingest(
                concept.embedding_text(), display_name=f"{concept.key}.txt"
            )
            concept_contents.append(ContentSelection(artifact=artifact.ref))
        if concept_contents:
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

    sequence_indptr, sequence_indices = _csr(source_rows)
    membership_indptr, membership_indices = _csr(candidate_memberships)
    path_step_indptr, path_indices = _csr(candidate_paths)
    path_indptr, _ = _csr([list(range(count)) for count in candidate_path_counts])
    candidate_root_indptr, candidate_root_indices = _csr(candidate_roots)
    group_root_indptr, group_root_indices = _csr(
        [[root_rows[root] for root in group_roots[group.key]] for group in manifest.support_groups]
    )
    return FrozenInputs(
        candidate_metadata={
            "keys": [item["key"] for item in candidate_mapping],
            "documents": [item["document"] for item in candidate_mapping],
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
            [group_rows[item.support_group] for item in manifest.associations], dtype=np.uint64
        ),
        group_root_indptr=group_root_indptr,
        group_root_indices=group_root_indices,
        vector_mappings=tuple(vector_mappings),
        mapping={
            "name": manifest.name,
            "metadata": manifest.metadata,
            "root_order": root_keys,
            "documents": {
                item.key: {
                    "artifact": accepted[item.key].model_dump(mode="json"),
                    "selector": item.input.selector.model_dump(mode="json"),
                    "roots": list(document_roots[item.key]),
                    "derived_from": list(item.derived_from),
                    "times": _document_times(item.times, accepted_artifacts[item.key]),
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
