"""A-Memorix v1 semantic projection, using its published rows and canonical digest.

Format and row projection follow A-Memorix's bundle_admin_service and Nous's
previous portable exporter. Native full records remain a declared supplement.
"""

from __future__ import annotations

import json
from datetime import UTC, datetime
from hashlib import sha256
from io import BytesIO
from zipfile import ZIP_DEFLATED, ZipFile

from nous_wave.contracts import (
    AssociationBody,
    BundleRequest,
    CognitiveRecord,
    DomainError,
    EntityBody,
    EpisodeBody,
    MemoryBody,
    SubjectBundle,
)
from pydantic import JsonValue, TypeAdapter

from nous.cognition.projection import evidence_in

TABLES = (
    "paragraphs",
    "entities",
    "relations",
    "deleted_relations",
    "paragraph_entities",
    "paragraph_relations",
    "external_memory_refs",
    "episodes",
    "episode_paragraphs",
    "fact_claims",
    "fact_evidence",
    "fact_transitions",
    "person_profile_snapshots",
    "person_profile_overrides",
    "person_profile_alias_overrides",
)


def canonical(value: object) -> bytes:
    return json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False
    ).encode()


def project(
    bundle: SubjectBundle,
) -> tuple[dict[str, JsonValue], dict[str, JsonValue], dict[str, int]]:
    heads: dict[str, CognitiveRecord] = {}
    for record in bundle.cognition:
        previous = heads.get(record.reference.id)
        if previous is None or record.reference.revision > previous.reference.revision:
            heads[record.reference.id] = record
    live = {
        event.id for event in bundle.observations if event.active and not event.record.get("purged")
    }
    selected = {
        key: record
        for key, record in heads.items()
        if not record.deleted
        and not record.purged
        and set(record.support_roots or record.source_roots).intersection(live)
    }
    memories = {
        key: record for key, record in selected.items() if isinstance(record.body, MemoryBody)
    }
    tables: dict[str, list[dict[str, JsonValue]]] = {name: [] for name in TABLES}
    entities: dict[str, EntityBody] = {}
    for key, record in memories.items():
        assert isinstance(record.body, MemoryBody)
        body = record.body
        metadata = {
            **body.metadata,
            "nous_reference": record.reference.model_dump(mode="json"),
            "nous_times": record.times.model_dump(mode="json"),
            "epistemic": body.epistemic.model_dump(mode="json"),
            "structured": body.structured,
            "external_ids": body.external_ids,
        }
        from nous.material.named_sources import source_name

        originals = {material.id: material.record for material in bundle.materials}
        labels = {
            label
            for event in bundle.observations
            if event.id in record.source_roots
            if (label := source_name(event.record, originals.get(event.material_id))) is not None
        }
        named = body.metadata.get("source")
        captured_source = (
            named if isinstance(named, str) else next(iter(labels)) if len(labels) == 1 else ""
        )
        row: dict[str, JsonValue] = {
            "hash": key,
            "content": body.text,
            "knowledge_type": str(body.metadata.get("knowledge_type") or "mixed"),
            "source": captured_source,
            "metadata": canonical(metadata).decode(),
            "is_deleted": 0,
            "is_permanent": int(bool(record.lifecycle and record.lifecycle.pinned)),
            "word_count": len(body.text),
        }
        for name, point in (
            ("created_at", record.times.formation),
            ("updated_at", record.times.recorded),
            ("event_time", record.times.event),
        ):
            if point is not None and point.domain == "unix_ms":
                row[name] = point.coordinate / 1000
        tables["paragraphs"].append(row)
    for key, record in selected.items():
        if not isinstance(record.body, EntityBody):
            continue
        body = record.body
        mentions = {mention.reference.exact() for mention in body.mentions}
        mentions.update(link.reference.exact() for link in evidence_in(record) if link.reference)
        paragraphs = [
            memory_id
            for memory_id, memory in memories.items()
            if memory.reference.exact() in mentions
        ]
        entities[key] = body
        tables["entities"].append(
            {
                "hash": key,
                "name": body.label,
                "is_deleted": 0,
                "metadata": canonical(
                    {
                        **body.model_dump(mode="json"),
                        "reference": record.reference.model_dump(mode="json"),
                    }
                ).decode(),
            }
        )
        tables["paragraph_entities"].extend(
            {"paragraph_hash": paragraph, "entity_hash": key} for paragraph in paragraphs
        )
    for key, record in selected.items():
        body = record.body
        if (
            isinstance(body, AssociationBody)
            and body.polarity == "positive"
            and body.from_ref.id in entities
            and body.to_ref.id in entities
        ):
            if (
                selected[body.from_ref.id].reference != body.from_ref
                or selected[body.to_ref.id].reference != body.to_ref
            ):
                continue
            paragraphs = [
                memory_id
                for memory_id, memory in memories.items()
                if any(link.reference == memory.reference for link in evidence_in(record))
            ]
            tables["relations"].append(
                {
                    "hash": key,
                    "subject": entities[body.from_ref.id].label,
                    "predicate": body.relation,
                    "object": entities[body.to_ref.id].label,
                    "is_inactive": 0,
                    "retention_strength": 1.0,
                    "retention_anchor_at": record.times.recorded.coordinate / 1000,
                    "metadata": canonical(body.model_dump(mode="json")).decode(),
                }
            )
            tables["paragraph_relations"].extend(
                {"paragraph_hash": paragraph, "relation_hash": key} for paragraph in paragraphs
            )
        elif isinstance(body, EpisodeBody) and body.state == "active":
            members = [
                (member.reference.id, member.ordinal)
                for member in body.members
                if member.reference.id in memories
                and memories[member.reference.id].reference == member.reference
            ]
            if not members:
                continue
            row = {
                "episode_id": key,
                "source": body.track,
                "title": body.title,
                "summary": body.summary,
                "paragraph_count": len(members),
                "created_at": record.times.recorded.coordinate / 1000,
                "updated_at": record.times.recorded.coordinate / 1000,
                "participants_json": "[]",
                "keywords_json": "[]",
                "evidence_ids_json": canonical([member for member, _ in members]).decode(),
            }
            if body.experience.domain == "unix_ms":
                row["event_time_start"] = (
                    body.experience.start / 1000 if body.experience.start is not None else None
                )
                row["event_time_end"] = (
                    body.experience.end / 1000 if body.experience.end is not None else None
                )
            tables["episodes"].append(TypeAdapter(dict[str, JsonValue]).validate_python(row))
            tables["episode_paragraphs"].extend(
                {"episode_id": key, "paragraph_hash": member, "position": ordinal}
                for member, ordinal in members
            )
    from .portable_facts import project_facts

    project_facts(bundle, tables, selected)
    from .portable_identity import semantic_ids

    identities = semantic_ids(bundle)
    for name, values in tables.items():
        for row in values:
            for field in ("hash", "paragraph_hash", "entity_hash", "relation_hash", "episode_id"):
                value = row.get(field)
                if isinstance(value, str) and value in identities:
                    row[field] = identities[value]
            if name == "episodes" and isinstance(row.get("evidence_ids_json"), str):
                row["evidence_ids_json"] = canonical(
                    [
                        identities.get(value, value)
                        for value in json.loads(str(row["evidence_ids_json"]))
                    ]
                ).decode()
        if name in {"paragraphs", "entities", "relations"} and len(
            {str(row["hash"]) for row in values}
        ) != len(values):
            raise DomainError(
                "bundle_portable_identity_collision",
                "The selected Native identities collapse to one A-Memorix semantic key; select a narrower scope",
                table=name,
            )
    docs: list[dict[str, JsonValue]] = []
    for paragraph in tables["paragraphs"]:
        key = str(paragraph["hash"])
        names = sorted(
            {
                str(entity["name"])
                for link in tables["paragraph_entities"]
                if link["paragraph_hash"] == key
                for entity in tables["entities"]
                if entity["hash"] == link["entity_hash"]
            }
        )
        triples = [
            [relation["subject"], relation["predicate"], relation["object"]]
            for link in tables["paragraph_relations"]
            if link["paragraph_hash"] == key
            for relation in tables["relations"]
            if relation["hash"] == link["relation_hash"]
        ]
        docs.append(
            TypeAdapter(dict[str, JsonValue]).validate_python(
                {
                    "id": key,
                    "passage": paragraph["content"],
                    "extracted_entities": names,
                    "extracted_triples": triples,
                    "a_memorix": {
                        "knowledge_type": paragraph["knowledge_type"],
                        "source": paragraph["source"],
                        "time_meta": {
                            name: value
                            for name, value in paragraph.items()
                            if name.startswith("event_time")
                        },
                        "metadata": json.loads(str(paragraph["metadata"])),
                    },
                }
            )
        )
    knowledge: dict[str, JsonValue] = {
        "format": "lpmm_openie",
        "format_version": 1,
        "docs": TypeAdapter(list[JsonValue]).validate_python(docs),
    }
    state: dict[str, JsonValue] = {
        "schema": "a-memorix-semantic-state",
        "schema_version": 1,
        "tables": TypeAdapter(dict[str, JsonValue]).validate_python(tables),
        "excluded": [
            "Derived search indexes and workflow execution are rebuilt by the receiving authority"
        ],
    }
    return knowledge, state, {name: len(rows) for name, rows in tables.items()}


def docs_from_knowledge(knowledge: dict[str, JsonValue]) -> list[dict[str, JsonValue]]:
    return TypeAdapter(list[dict[str, JsonValue]]).validate_python(knowledge["docs"])


def write_portable(bundle: SubjectBundle, command: BundleRequest) -> bytes:
    knowledge, state, counts = project(bundle)
    from .portable_images import project_images
    from .portable_read import image_semantics

    portable_ids = {str(doc["id"]) for doc in docs_from_knowledge(knowledge)}
    table_values = state.get("tables")
    if isinstance(table_values, dict):
        selected_tables = (
            (("entities", "hash"), ("relations", "hash"), ("episodes", "episode_id"))
            if command.content_level == "full"
            else (("paragraph_entities", "entity_hash"), ("paragraph_relations", "relation_hash"))
        )
        for table, field in selected_tables:
            values = table_values.get(table)
            if isinstance(values, list):
                portable_ids.update(
                    str(value[field]) for value in values if isinstance(value, dict)
                )
    images, image_members = (
        project_images(bundle, portable_ids)
        if command.assets is None or command.assets.include_images
        else (None, {})
    )
    selected_state = state if command.content_level == "full" else None
    members = {"knowledge.json": canonical(knowledge)}
    if selected_state is not None:
        members["state.json"] = canonical(selected_state)
        if command.assets is None or command.assets.include_images:
            members["bundle.json"] = canonical(bundle.model_dump(mode="json"))
    from .portable_vectors import export_vectors

    vector_members, fingerprint, image_fingerprint = export_vectors(
        bundle, command, images, portable_ids
    )
    members.update(vector_members)
    members.update(image_members)
    if images is not None:
        members["images.json"] = canonical(images)
    has_semantic_state = selected_state is not None and any(
        counts[name]
        for name in (
            "entities",
            "relations",
            "episodes",
            "fact_claims",
            "person_profile_snapshots",
            "person_profile_overrides",
            "person_profile_alias_overrides",
        )
    )
    if not knowledge["docs"] and images is None and not has_semantic_state:
        raise DomainError(
            "bundle_portable_empty", "The selected scope contains no portable semantic records"
        )
    content: dict[str, JsonValue] = {"knowledge": knowledge, "state": selected_state}
    if images is not None:
        content["images"] = image_semantics(images)
    from nous_wave.contracts.bundles import PackageDescriptor

    semantic_digest = sha256(canonical(content)).hexdigest()
    package = bundle.package or PackageDescriptor(
        id="amx." + semantic_digest[:16], name="A_Memorix 知识包", version="1.0.0"
    )
    package = package.model_copy(update={"version": package.version or "1.0.0"})
    if "bundle.json" in members:
        members["bundle.json"] = canonical(
            bundle.model_copy(update={"package": package}).model_dump(mode="json")
        )
    if images is not None:
        for field in ("assets", "occurrences", "observations", "links"):
            values = images.get(field)
            counts["image_" + field] = len(values) if isinstance(values, list) else 0
    from .portable_vectors import coordinates

    counts["vector_assets"] = sum(len(coordinates(data)[0]) for data in vector_members.values())
    manifest = {
        "format": "a-memorix-memory-bundle",
        "format_version": 2 if images is not None else 1,
        "created_at": datetime.now(UTC).isoformat(),
        "package": package.model_dump(mode="json"),
        "content_level": command.content_level,
        "content_digest": semantic_digest,
        "selector": command.selector.model_dump(mode="json"),
        "counts": counts,
        "components": sorted(members),
        "embedding_fingerprint": fingerprint,
        "image_embedding_fingerprint": image_fingerprint,
        "retention": {
            "nous_native": {"member": "bundle.json", "status": "included"}
            if "bundle.json" in members
            else {"status": "excluded_from_knowledge"}
        },
    }
    members["manifest.json"] = canonical(manifest)
    members["checksums.json"] = canonical(
        {
            "algorithm": "sha256",
            "files": {name: sha256(data).hexdigest() for name, data in members.items()},
        }
    )
    output = BytesIO()
    with ZipFile(output, "w", ZIP_DEFLATED) as archive:
        for name, data in sorted(members.items()):
            archive.writestr(name, data)
    return output.getvalue()
