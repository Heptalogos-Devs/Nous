"""Serialization and validation of recoverable Wave snapshot manifests."""

from __future__ import annotations

import hashlib
import json
import os
from collections.abc import Sequence
from pathlib import Path
from typing import cast
from uuid import uuid4

from pydantic import JsonValue, TypeAdapter

from nous.common.errors import NousError

from .contracts import PreparedSnapshot, ProjectionUnit


def encode_json(value: object) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def content_signature(value: object) -> str:
    return hashlib.sha256(encode_json(value).encode("utf-8")).hexdigest()


def manifest_references(path: Path) -> tuple[str, set[str], str]:
    envelope = json.loads((path / "wave-manifest.json").read_text(encoding="utf-8"))
    value = envelope["prepared"]
    if envelope["digest"] != content_signature(value):
        raise NousError("wave.manifest_invalid", "Prepared manifest checksum differs")
    return (
        value["index_location"],
        {item["signature"] for item in value["unit_refs"]},
        value["graph_signature"],
    )


def write_unit(root: Path, unit: ProjectionUnit) -> None:
    data = unit.model_dump(mode="json", exclude={"signature", "candidate_rows"})
    write_shared(root, unit.signature, data, prefix="")


def write_units(root: Path, units: Sequence[ProjectionUnit]) -> None:
    """Validate or publish a bounded group of immutable projection assets."""
    for unit in units:
        write_unit(root, unit)


def write_shared(root: Path, signature: str, data: object, *, prefix: str) -> None:
    if content_signature(data) != signature:
        raise NousError("wave.manifest_invalid", "Projection asset signature differs")
    path = root / f"{prefix}{signature}.json"
    if path.exists():
        stored = path.read_bytes()
        # Shared assets are written in canonical JSON. A raw digest verifies the
        # common case without parsing and serializing every unchanged unit again.
        if (
            hashlib.sha256(stored).hexdigest() != signature
            and content_signature(json.loads(stored.decode("utf-8"))) != signature
        ):
            raise NousError("wave.manifest_invalid", "Stored projection asset differs")
        return
    staging = root / f".staging-{prefix}{signature}-{uuid4()}"
    try:
        staging.write_text(encode_json(data), encoding="utf-8")
        os.replace(staging, path)
    finally:
        if staging.exists():
            staging.unlink()


def manifest_value(prepared: PreparedSnapshot) -> dict[str, object]:
    data = prepared.model_dump(mode="json", exclude={"units", "graph_payload", "vector_mappings"})
    view = data["view"]
    documents = view.pop("documents")
    candidates = view.pop("candidates")
    evidence = view.pop("evidence")
    data["source_documents"] = evidence["source_documents"]
    data["sequence_documents"] = [item["document"] for item in view.pop("source_sequences")]
    for name in ("concepts", "associations", "support_groups"):
        view.pop(name)
    layout = data["layout"]
    for name in (
        "name",
        "owner",
        "metadata",
        "roots",
        "concepts",
        "associations",
        "support_groups",
    ):
        layout.pop(name)
    data["document_order"] = [item["key"] for item in documents]
    data["candidate_order"] = [
        item["key"] if item["kind"] == "exact" else item["group_key"] for item in candidates
    ]
    data["unit_refs"] = [
        {"signature": unit.signature, "candidate_rows": list(unit.candidate_rows)}
        for unit in prepared.units
    ]
    return data


def restore_prepared(path: Path, unit_root: Path) -> PreparedSnapshot:
    envelope = json.loads((path / "wave-manifest.json").read_text(encoding="utf-8"))
    data = envelope["prepared"]
    if envelope["digest"] != content_signature(data):
        raise NousError("wave.manifest_invalid", "Prepared manifest checksum differs")
    references = data.pop("unit_refs")
    document_order = data.pop("document_order")
    candidate_order = data.pop("candidate_order")
    source_documents = data.pop("source_documents")
    sequence_documents = data.pop("sequence_documents")
    graph = json.loads(
        (unit_root / f"graph-{data['graph_signature']}.json").read_text(encoding="utf-8")
    )
    if content_signature(graph) != data["graph_signature"]:
        raise NousError("wave.manifest_invalid", "Projection graph checksum differs")
    units: list[ProjectionUnit] = []
    documents: dict[str, object] = {}
    candidates: dict[str, object] = {}
    roots: dict[str, object] = {}
    document_roots: dict[str, object] = {}
    candidate_vectors: dict[str, dict[str, JsonValue]] = {}
    candidate_rows: list[tuple[int, str]] = []
    sequences: dict[str, list[dict[str, JsonValue]]] = {}
    for reference in references:
        signature = reference["signature"]
        unit_data = json.loads((unit_root / f"{signature}.json").read_text(encoding="utf-8"))
        if content_signature(unit_data) != signature:
            raise NousError("wave.manifest_invalid", "Projection unit checksum differs")
        unit = ProjectionUnit.model_validate({**unit_data, **reference})
        if content_signature(unit.input_payload) != unit.input_signature:
            raise NousError("wave.manifest_invalid", "Projection input signature differs")
        units.append(unit)
        for vector in unit.vector_mappings:
            candidate_vectors[cast(str, vector["key"])] = vector
        candidate_rows.extend(
            (row, cast(str, candidate["key"]))
            for row, candidate in zip(unit.candidate_rows, unit.candidates, strict=True)
        )
        roots.update(TypeAdapter(dict[str, JsonValue]).validate_python(unit.input_payload["roots"]))
        document_roots.update(
            TypeAdapter(dict[str, JsonValue]).validate_python(unit.input_payload["document_roots"])
        )
        for item in TypeAdapter(list[dict[str, JsonValue]]).validate_python(
            unit.input_payload["documents"]
        ):
            documents[cast(str, item["key"])] = item
        for item in TypeAdapter(list[dict[str, JsonValue]]).validate_python(
            unit.input_payload["declarations"]
        ):
            key = item["key"] if item["kind"] == "exact" else item["group_key"]
            candidates[cast(str, key)] = item
        sequences[unit.document_key] = [
            item
            for item in TypeAdapter(list[dict[str, JsonValue]]).validate_python(
                unit.input_payload["sequences"]
            )
            if item["document"] == unit.document_key
        ]
    data["view"]["documents"] = [documents[key] for key in document_order]
    data["view"]["candidates"] = [candidates[key] for key in candidate_order]
    positions: dict[str, int] = {}
    source_sequences: list[dict[str, JsonValue]] = []
    for document in sequence_documents:
        index = positions.get(document, 0)
        source_sequences.append(sequences[document][index])
        positions[document] = index + 1
    data["view"]["source_sequences"] = source_sequences
    data["view"].update(graph["view"])
    data["view"]["evidence"] = {
        "roots": roots,
        "documents": document_roots,
        "source_documents": source_documents,
    }
    data["layout"].update(
        {
            "name": data["view"]["name"],
            "owner": data["view"]["owner"],
            "metadata": data["view"]["metadata"],
            "roots": roots,
            **graph["layout"],
        }
    )
    data["vector_mappings"] = [
        candidate_vectors[key] for _, key in sorted(candidate_rows) if key in candidate_vectors
    ] + graph["vector_mappings"]
    data["graph_payload"] = graph
    data["units"] = units
    prepared = PreparedSnapshot.model_validate(data)
    if (
        content_signature(
            {
                "view": prepared.view.model_dump(mode="json"),
                "profile": prepared.profile.model_dump(mode="json"),
            }
        )
        != prepared.build_signature
    ):
        raise NousError("wave.manifest_invalid", "Prepared input signature differs")
    if (
        prepared.identity.get("component_signature") != prepared.component_signature
        or prepared.index_location != f"wave/components/{prepared.component_signature}"
        or [row for row, _ in sorted(candidate_rows)] != list(range(len(candidate_rows)))
    ):
        raise NousError("wave.manifest_invalid", "Prepared component or row mapping differs")
    return prepared
