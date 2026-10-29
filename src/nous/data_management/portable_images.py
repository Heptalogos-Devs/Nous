"""Published A-Memorix image extension from actual retained bytes and lineage."""

from __future__ import annotations

import base64

from nous_wave.contracts import DomainError, SubjectBundle
from nous_wave.contracts.derivation import TextRepresentation
from pydantic import JsonValue, TypeAdapter

from .portable_read import rows

_OBJECT = TypeAdapter(dict[str, JsonValue])


def project_images(
    bundle: SubjectBundle, portable_ids: set[str]
) -> tuple[dict[str, JsonValue] | None, dict[str, bytes]]:
    from .portable_identity import semantic_ids

    identities = semantic_ids(bundle)
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
    imported_descriptions: set[str] = set()
    for event in bundle.observations:
        raw = event.record.get("amemorix_image")
        if event.id not in occurrence_ids or not isinstance(raw, dict):
            continue
        values = raw.get("observations")
        if isinstance(values, list):
            for value in values:
                if isinstance(value, dict):
                    observations.append({**value, "occurrence_id": event.id})
                    imported_descriptions.add(event.id)
    for representation in latest.values():
        if representation.kind != "image_description" or not isinstance(
            representation.payload, TextRepresentation
        ):
            continue
        for root in representation.source_roots:
            if root in occurrence_ids and not (
                root in imported_descriptions
                and representation.producer.implementation == "amemorix:imported-image-observation"
            ):
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
    heads = {
        record.reference.id: record
        for record in sorted(bundle.cognition, key=lambda record: record.reference.revision)
    }
    kinds = {
        "memory": "paragraph",
        "entity": "entity",
        "association": "relation",
        "episode": "episode",
    }
    for event in bundle.observations:
        if event.id not in occurrence_ids:
            continue
        raw = event.record.get("amemorix_image")
        external = raw.get("links") if isinstance(raw, dict) else None
        emitted: set[str] = set()
        for link in rows(external):
            target = link.get("target_reference")
            if not isinstance(target, dict):
                continue
            value = heads.get(str(target.get("id")))
            if (
                value is None
                or identities.get(value.reference.id) not in portable_ids
                or value.deleted
                or value.purged
                or value.reference.revision != target.get("revision")
            ):
                continue
            links.append(
                {
                    **{key: item for key, item in link.items() if key != "target_reference"},
                    "occurrence_id": event.id,
                    "target_id": identities[value.reference.id],
                }
            )
            emitted.add(value.reference.id)
        for value in heads.values():
            if (
                value.reference.id in emitted
                or identities.get(value.reference.id) not in portable_ids
                or value.deleted
                or value.purged
                or event.id not in value.source_roots
                or value.reference.kind not in kinds
            ):
                continue
            links.append(
                {
                    "occurrence_id": event.id,
                    "target_type": kinds[value.reference.kind],
                    "target_id": identities[value.reference.id],
                    "link_kind": "source",
                    "evidence_json": {"nous_reference": value.reference.exact()},
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
