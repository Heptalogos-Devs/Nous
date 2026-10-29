"""Published A-Memorix image extension from actual retained bytes and lineage."""

from __future__ import annotations

import base64

from nous_wave.contracts import DomainError, MemoryBody, SubjectBundle
from nous_wave.contracts.derivation import TextRepresentation
from pydantic import JsonValue, TypeAdapter

_OBJECT = TypeAdapter(dict[str, JsonValue])


def project_images(bundle: SubjectBundle) -> tuple[dict[str, JsonValue] | None, dict[str, bytes]]:
    material_rows = {row.id: row for row in bundle.materials}
    image_rows = {
        key: row
        for key, row in material_rows.items()
        if str(row.record.get("media_type") or "").startswith("image/")
        and not row.record.get("purged")
    }
    assets: dict[str, dict[str, JsonValue]] = {}
    members: dict[str, bytes] = {}
    occurrences: list[dict[str, JsonValue]] = []
    observations: list[dict[str, JsonValue]] = []
    links: list[dict[str, JsonValue]] = []
    occurrence_ids: set[str] = set()
    for event in bundle.observations:
        material = image_rows.get(event.material_id)
        if material is None or not event.active or event.record.get("purged"):
            continue
        digest = material.content_hash
        member = f"images/assets/{digest}"
        encoded = bundle.contents.get(digest)
        if encoded is None:
            raise DomainError(
                "bundle_content_missing", "Image export requires its retained original bytes"
            )
        members[member] = base64.b64decode(encoded, validate=True)
        info = material.record.get("media_info")
        info = info if isinstance(info, dict) else {}
        assets[digest] = {
            "asset_id": digest,
            "content_hash": digest,
            "member": member,
            "byte_size": len(members[member]),
            "mime_type": str(material.record["media_type"]),
            "width": info.get("width"),
            "height": info.get("height"),
        }
        occurrences.append(
            {
                "occurrence_id": event.id,
                "asset_id": digest,
                "component_path": str(material.record.get("filename") or ""),
                "recorded_at": event.record.get("recorded_at"),
                "observed_at": event.record.get("observed_at"),
                "origin": event.record.get("origin"),
            }
        )
        occurrence_ids.add(event.id)
    if not assets:
        return None, {}
    latest = {
        record.reference.id: record
        for record in sorted(
            bundle.derivations.representations, key=lambda record: record.reference.revision
        )
    }
    for representation in latest.values():
        if representation.kind != "image_description" or not isinstance(
            representation.payload, TextRepresentation
        ):
            continue
        for root in representation.source_roots:
            if root in occurrence_ids:
                observations.append(
                    {
                        "occurrence_id": root,
                        "text": representation.payload.text,
                        "source_kind": "model_description"
                        if representation.producer.model_call_id
                        else "imported_description",
                        "confirm_status": "unconfirmed",
                        "evidence_json": {
                            "representation_id": representation.reference.id,
                            "revision": representation.reference.revision,
                            "producer": representation.producer.model_dump(mode="json"),
                        },
                    }
                )
    memory_heads = {
        record.reference.id: record
        for record in sorted(bundle.cognition, key=lambda record: record.reference.revision)
        if isinstance(record.body, MemoryBody)
    }
    for memory in memory_heads.values():
        if memory.deleted or memory.purged:
            continue
        for root in memory.source_roots:
            if root in occurrence_ids:
                links.append(
                    {
                        "occurrence_id": root,
                        "target_type": "paragraph",
                        "target_id": memory.reference.id,
                        "link_kind": "source",
                        "evidence_json": {"nous_reference": memory.reference.exact()},
                    }
                )
    return _OBJECT.validate_python(
        {
            "schema": "a-memorix-image-state",
            "schema_version": 1,
            "assets": list(assets.values()),
            "occurrences": occurrences,
            "observations": observations,
            "links": links,
        }
    ), members
