"""Resolve collection inputs, provenance and immutable Native array bundles."""

from __future__ import annotations

import hashlib
from dataclasses import dataclass
from time import perf_counter
from typing import cast

import numpy as np
import numpy.typing as npt
from pydantic import JsonValue

from nous.application.embedding import EmbeddingOperations
from nous.application.preparation import PreparedText, TextPreparation
from nous.common.errors import NousError
from nous.content.annotations import AnnotationSet, read_annotation
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import (
    Artifact,
    ArtifactRef,
    ContentSelection,
)
from nous.content.text import TextView
from nous.media.preparation import MediaPreparer
from nous.temporal.values import CognitiveTimes, Instant, parse_explicit_time

from .candidate_inputs import covers, prepare_candidates, titled_semantics
from .contracts import (
    CandidatePath,
    CognitionOwner,
    CorpusView,
    PathStep,
    SnapshotOwner,
)
from .mapping import AssociationMapping, CandidateMapping, ConceptMapping, SnapshotMapping


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


async def freeze_inputs(
    manifest: CorpusView,
    space_id: str | None,
    artifacts: ArtifactStore,
    embedding: EmbeddingOperations,
    preparer: MediaPreparer,
    cached_units: dict[str, list[dict[str, object]]],
    cached_layouts: dict[str, list[dict[str, object]]],
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
    source_documents = set(manifest.evidence.source_documents)
    for index, sequence in enumerate(manifest.source_sequences):
        document = documents[sequence.document]
        if sequence.document not in source_documents:
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
    async for candidate, prepared, cached in prepare_candidates(
        manifest, documents, accepted, preparation, cached_units, cached_layouts
    ):
        if cached is not None and candidate is None:
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
        assert candidate is not None
        document = documents[candidate.content.document]
        if cached is None:
            assert prepared is not None
            origin_contents = prepared.origin_contents
            selection = prepared.view.body
            text = prepared.text
        else:
            previous = cast(CandidateMapping, cached["mapping"])
            selection = ContentSelection.model_validate(previous["content"])
            origin_contents = tuple(
                ContentSelection.model_validate(value) for value in previous["origin_contents"]
            )
            text = cast(str, cached["text"])
        if not text.strip():
            continue
        candidate_contents.append(selection)
        if cached is None:
            assert prepared is not None
            fresh_indices.append(len(candidate_mapping))
            semantic_slots.append(None)
            prepared_candidates.append(prepared)
            retrieval_content = selection
        else:
            retrieval_content = ContentSelection.model_validate(
                cast(CandidateMapping, cached["mapping"])["retrieval_content"]
            )
            semantic_slots.append(retrieval_content)
        candidate_texts.append(text)
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
                if await covers(selection, occurrence.selections, preparation):
                    covered_concepts.append(occurrence.concept)
            annotation_memberships = tuple(dict.fromkeys(covered_concepts))
        if candidate.annotation_ref is not None:
            assert prepared is not None
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
                inside = await covers(selection, step.selections, preparation)
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
                "local": not candidate.content.selector.is_whole,
                "roots": list(roots_for_candidate),
                "concepts": list(dict.fromkeys(memberships)),
                "paths": path_mapping,
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
            await titled_semantics(
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
            "reused_layout_units": len(cached_layouts),
        },
        candidate_metadata={
            "keys": [item["key"] for item in candidate_mapping],
            "documents": [item["document"] for item in candidate_mapping],
            "titles": indexed_titles,
            "group_keys": group_keys,
            "group_rows": [candidate_group_rows[item["group_key"]] for item in candidate_mapping],
            "local": [item["local"] for item in candidate_mapping],
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
            "groups": {},
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
