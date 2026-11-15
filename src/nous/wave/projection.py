"""Projection input identities and source evidence for imported views."""

from collections.abc import Sequence
from typing import cast

from pydantic import JsonValue

from nous.common.errors import NousError
from nous.content.contracts import ArtifactRef
from nous.content.results import ResultStore
from nous.content.text import COMPLETE_BLOCKS_LAYOUT

from .contracts import (
    Candidate,
    CorpusView,
    Document,
    DocumentCandidates,
    EvidenceProjection,
    SourceSequence,
    document_order,
)


def layout_inputs(
    view: CorpusView, preparations: dict[str, dict[str, JsonValue]]
) -> dict[str, dict[str, JsonValue]]:
    """Identify reusable text and layout independently of title and annotation."""
    documents = {value.key: value for value in view.documents}
    declarations: dict[str, list[Candidate | DocumentCandidates]] = {}
    for value in view.candidates:
        declarations.setdefault(value.document, []).append(value)
    result: dict[str, dict[str, JsonValue]] = {}
    for key, values in declarations.items():
        if len(values) != 1 or not isinstance(values[0], DocumentCandidates):
            continue
        declaration = values[0]
        if declaration.annotation_ref is not None:
            continue
        result[key] = {
            "source": documents[key].input.model_dump(mode="json"),
            "preparation": preparations[key],
            "whole": declaration.whole,
            "layout": declaration.local_layout.model_dump(mode="json")
            if declaration.local_layout
            else None,
            "layout_policy": COMPLETE_BLOCKS_LAYOUT,
        }
    return result


def projection_inputs(view: CorpusView) -> dict[str, dict[str, object]]:
    """Separate exact per-document inputs from the small snapshot-wide view."""
    documents = {document.key: document for document in view.documents}
    declarations_by_document: dict[str, list[Candidate | DocumentCandidates]] = {}
    for declaration in view.candidates:
        declarations_by_document.setdefault(declaration.document, []).append(declaration)
    sequences_by_document: dict[str, list[tuple[int, SourceSequence]]] = {}
    for index, sequence in enumerate(view.source_sequences):
        sequences_by_document.setdefault(sequence.document, []).append((index, sequence))
    result: dict[str, dict[str, object]] = {}
    for document in view.documents:
        declarations = declarations_by_document.get(document.key, [])
        sources = {
            item.content.document if isinstance(item, Candidate) else item.document
            for item in declarations
        } | {document.key}
        roots = {root for key in sources for root in view.evidence.documents[key]}
        payload = {
            "layout_policy": COMPLETE_BLOCKS_LAYOUT
            if any(
                isinstance(item, DocumentCandidates) and item.local_layout for item in declarations
            )
            else None,
            "documents": [
                {
                    "key": key,
                    "input": documents[key].input.model_dump(mode="json"),
                    "title": documents[key].title,
                }
                for key in sorted(sources)
            ],
            "declarations": [item.model_dump(mode="json") for item in declarations],
            "sequences": [
                item.model_dump(mode="json")
                for _, item in sorted(
                    (entry for key in sources for entry in sequences_by_document.get(key, [])),
                    key=lambda entry: entry[0],
                )
            ],
            "document_roots": {key: list(view.evidence.documents[key]) for key in sorted(sources)},
            "roots": {
                key: [ref.model_dump(mode="json") for ref in view.evidence.roots[key]]
                for key in sorted(roots)
            },
        }
        result[document.key] = cast(dict[str, object], payload)
    return result


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
