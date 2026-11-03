"""Resolve collection inputs, provenance and immutable Native array bundles."""

from __future__ import annotations

import hashlib
from dataclasses import dataclass
from pathlib import Path
from typing import cast

import numpy as np
import numpy.typing as npt
from pydantic import JsonValue
from typing_extensions import TypedDict

from nous.application.embedding import EmbeddingOperations
from nous.common.errors import NousError
from nous.content.contracts import (
    Artifact,
    ArtifactRef,
    ContentSelection,
    InputSpec,
    OutputContent,
    SourceSelection,
)
from nous.content.store import ArtifactStore, ResultStore
from nous.media.preparation import MediaPreparer
from nous.temporal.values import CognitiveTimes, Instant, parse_explicit_time

from .contracts import CollectionManifest, Selector


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
    content_document: str
    document: str
    roots: list[str]
    concepts: list[str]
    curve: list[str]
    metadata: dict[str, JsonValue]


class AssociationMapping(TypedDict):
    key: str
    source: str
    target: str
    relation: str
    directed: bool
    strength: float
    confidence: float
    polarity: str
    evidence: list[str]
    roots: list[str]


class SnapshotMapping(TypedDict):
    name: str
    root_order: list[str]
    documents: dict[str, DocumentMapping]
    roots: dict[str, list[dict[str, str]]]
    concepts: list[ConceptMapping]
    candidates: list[CandidateMapping]
    associations: list[AssociationMapping]


class SnapshotRow(TypedDict):
    id: str
    address: str
    collection_id: str
    manifest_artifact_id: str
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
    curve_indptr: npt.NDArray[np.uint64]
    curve_indices: npt.NDArray[np.uint64]
    curve_positions: npt.NDArray[np.int64]
    candidate_root_indptr: npt.NDArray[np.uint64]
    candidate_root_indices: npt.NDArray[np.uint64]
    association_metadata: dict[str, object]
    association_sources: npt.NDArray[np.uint64]
    association_targets: npt.NDArray[np.uint64]
    association_directed: npt.NDArray[np.bool_]
    association_strengths: npt.NDArray[np.float64]
    association_confidences: npt.NDArray[np.float64]
    association_polarities: npt.NDArray[np.int8]
    association_root_indptr: npt.NDArray[np.uint64]
    association_root_indices: npt.NDArray[np.uint64]
    vector_mappings: tuple[dict[str, object], ...]
    mapping: SnapshotMapping


def _csr(rows: list[list[int]]) -> tuple[npt.NDArray[np.uint64], npt.NDArray[np.uint64]]:
    offsets = [0]
    values: list[int] = []
    for row in rows:
        values.extend(row)
        offsets.append(len(values))
    return np.asarray(offsets, dtype=np.uint64), np.asarray(values, dtype=np.uint64)


def _candidate_selection(base: Selector, selected: Selector) -> SourceSelection:
    if selected.kind == "whole":
        return base.content_selection()
    if base.kind == "whole":
        return selected.content_selection()
    if base.kind != selected.kind or base.kind not in {"lines", "bytes"}:
        raise NousError(
            "wave.invalid_selection", "A nested selection must use the document's coordinate kind"
        )
    low = base.first if base.kind == "lines" else base.start
    high = base.last if base.kind == "lines" else base.end
    sub_low = selected.first if base.kind == "lines" else selected.start
    sub_high = selected.last if base.kind == "lines" else selected.end
    if low is not None and (sub_low is None or sub_low < low):
        raise NousError("wave.invalid_selection", "Candidate starts outside its declared document")
    if high is not None and (sub_high is None or sub_high > high):
        raise NousError("wave.invalid_selection", "Candidate ends outside its declared document")
    return selected.content_selection()


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


async def _searchable_content(
    origin: ContentSelection,
    artifacts: ArtifactStore,
    results: ResultStore,
    preparer: MediaPreparer,
) -> ContentSelection:
    """Resolve searchable text through its actual content and deterministic media owners."""
    artifact = await artifacts.get(origin.artifact)
    if artifact.media_type == "application/pdf":
        prepared = await preparer.prepare(
            artifact,
            origin.selection,
            requirements=("text/plain",),
            settings=preparer.execution.snapshot.typed_values().media,
            document_mode="text",
        )
        groups = tuple(group for batch in prepared.batches for group in batch.inputs)
        parts = [part for group in groups for part in group.parts if part.representation == "text"]
        if len(parts) == 1:
            return parts[0].content
        text = "\n".join([await artifacts.read_selection(part.content) for part in parts])
        if not parts:
            raise NousError("wave.text_required", "Selected PDF pages contain no extractable text")
        cache_key = hashlib.sha256(origin.model_dump_json().encode()).hexdigest()
        cached = await results.find_cached("wave.prepare.pdf-text", cache_key)
        if cached is None:
            cached = await results.adopt(
                operation="wave.prepare.pdf-text",
                producer={"kind": "deterministic", "tool": "PDFium text pages"},
                inputs=groups,
                outputs={
                    "text": OutputContent(
                        data=text.encode(),
                        media_type="text/plain",
                        display_name="selected-pages.txt",
                    )
                },
                parameters={"cache_key": cache_key},
                coverage=prepared.coverage,
            )
        output = next(output for output in cached.outputs if output.name == "text")
        return ContentSelection(artifact=output.artifact)
    if artifact.media_type.split("/", 1)[0] in {"image", "audio", "video"}:
        raise NousError(
            "wave.text_required",
            "Wave indexes text; provide an explicit transcript or media-analysis text artifact",
            details={
                "artifact": artifact.ref.address,
                "media_type": artifact.media_type,
                "selection": origin.selection.model_dump(mode="json"),
            },
        )
    return origin


async def freeze_inputs(
    manifest: CollectionManifest,
    base_dir: Path,
    space_id: str | None,
    artifacts: ArtifactStore,
    results: ResultStore,
    embedding: EmbeddingOperations,
    preparer: MediaPreparer,
) -> FrozenInputs:
    """Admit documents once and preserve independent roots through all derived candidates."""
    documents = {item.key: item for item in manifest.documents}
    accepted: dict[str, ArtifactRef] = {}
    accepted_artifacts: dict[str, Artifact] = {}
    for document in manifest.documents:
        source = document.input
        if source.file is not None:
            path = Path(source.file)
            path = path if path.is_absolute() else base_dir / path
            spec = InputSpec(kind="file", path=str(path.resolve()))
        elif source.text is not None:
            spec = InputSpec(kind="text", text=source.text, display_name=document.key)
        else:
            assert source.artifact is not None
            spec = InputSpec(kind="artifact", artifact=source.artifact)
        artifact = await artifacts.accept(spec)
        accepted[document.key] = artifact.ref
        accepted_artifacts[document.key] = artifact

    provenance = await results.provenance(tuple(accepted.values()))
    root_artifacts: dict[str, list[ArtifactRef]] = {}
    document_roots: dict[str, tuple[str, ...]] = {}

    def roots(key: str) -> tuple[str, ...]:
        if key in document_roots:
            return document_roots[key]
        document = documents[key]
        if document.derived_from:
            keys = tuple(
                dict.fromkeys(root for parent in document.derived_from for root in roots(parent))
            )
        else:
            ref = accepted[key]
            origins = provenance.get(ref.id, (ref,))
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

    for key in documents:
        roots(key)
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
        # Only terminal original sources add facts; derived documents are still candidates.
        ref = accepted[sequence.document]
        origins = provenance.get(ref.id, (ref,))
        if document.derived_from or len(origins) != 1 or origins[0].id != ref.id:
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
    candidate_memberships: list[list[int]] = []
    candidate_curves: list[list[int]] = []
    candidate_roots: list[list[int]] = []
    candidate_mapping: list[CandidateMapping] = []
    for candidate in manifest.candidates:
        document = documents[candidate.content.document]
        selection = ContentSelection(
            artifact=accepted[candidate.content.document],
            selection=_candidate_selection(document.input.selector, candidate.content.selector),
        )
        origin_content = selection
        selection = await _searchable_content(origin_content, artifacts, results, preparer)
        candidate_contents.append(selection)
        candidate_texts.append(await artifacts.read_selection(selection))
        inherited = document_sequences.get(candidate.content.document, [])
        can_inherit = (
            candidate.content.selector.kind == "whole"
            and document.input.selector.kind == "whole"
            and len(document_roots[candidate.content.document]) == 1
            and len(inherited) == 1
        )
        curve = candidate.curve
        if curve is None and can_inherit:
            # Unordered facts may supply membership, never a fabricated curve.
            if inherited[0][1]:
                curve = inherited[0][0]
        memberships = candidate.concepts
        if memberships is None:
            memberships = tuple(dict.fromkeys(curve or (inherited[0][0] if can_inherit else ())))
        curve = curve or ()
        candidate_curves.append([concept_rows[key] for key in curve])
        candidate_memberships.append([concept_rows[key] for key in dict.fromkeys(memberships)])
        roots_for_candidate = document_roots[candidate.content.document]
        candidate_roots.append([root_rows[key] for key in roots_for_candidate])
        candidate_mapping.append(
            {
                "key": candidate.key,
                "content": selection.model_dump(mode="json"),
                "origin_content": origin_content.model_dump(mode="json"),
                "content_document": candidate.content.document,
                "document": candidate.document,
                "roots": list(roots_for_candidate),
                "concepts": list(dict.fromkeys(memberships)),
                "curve": list(curve),
                "metadata": candidate.metadata,
            }
        )

    association_roots: list[list[int]] = []
    association_mapping: list[AssociationMapping] = []
    for association in manifest.associations:
        evidence = tuple(
            dict.fromkeys(
                root for document in association.evidence for root in document_roots[document]
            )
        )
        association_roots.append([root_rows[root] for root in evidence])
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
            batch = await embedding.ensure(candidate_contents, space_id, "document")
            candidate_vectors = await embedding.read(batch)
            vector_mappings.extend(
                {
                    "kind": "candidate",
                    "key": candidate.key,
                    "input": source.model_dump(mode="json"),
                    "result_id": row.result_id,
                    "row_index": row.row_index,
                }
                for candidate, source, row in zip(
                    manifest.candidates, batch.inputs, batch.rows, strict=True
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
    curve_indptr, curve_indices = _csr(candidate_curves)
    candidate_root_indptr, candidate_root_indices = _csr(candidate_roots)
    association_root_indptr, association_root_indices = _csr(association_roots)
    return FrozenInputs(
        candidate_metadata={
            "keys": [item.key for item in manifest.candidates],
            "documents": [item.document for item in manifest.candidates],
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
        curve_indptr=curve_indptr,
        curve_indices=curve_indices,
        curve_positions=np.asarray(
            [position for row in candidate_curves for position in range(1, len(row) + 1)],
            dtype=np.int64,
        ),
        candidate_root_indptr=candidate_root_indptr,
        candidate_root_indices=candidate_root_indices,
        association_metadata={
            "keys": [item.key for item in manifest.associations],
            "relations": [item.relation for item in manifest.associations],
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
        association_root_indptr=association_root_indptr,
        association_root_indices=association_root_indices,
        vector_mappings=tuple(vector_mappings),
        mapping={
            "name": manifest.name,
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
        },
    )
