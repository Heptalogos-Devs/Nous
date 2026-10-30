"""Project external A-Memorix semantic rows into source-backed Nous revisions."""

from __future__ import annotations

import base64
import json
import math
from datetime import datetime
from hashlib import sha256
from uuid import NAMESPACE_URL, uuid5

from nous_wave.contracts import (
    AssociationBody,
    CognitiveBody,
    CognitiveRecord,
    CognitiveRef,
    CognitiveTimes,
    DomainError,
    EntityBody,
    EntityMention,
    EpisodeBody,
    EpisodeMember,
    EpistemicStance,
    EvidenceLink,
    MemoryBody,
    SubjectBundle,
    TimePoint,
    TimeRange,
)
from nous_wave.contracts.bundles import PackageDescriptor
from nous_wave.contracts.cognition import PersonProfileControls
from nous_wave.contracts.management import BundleMaterial, BundleObservation
from pydantic import JsonValue, TypeAdapter

from .portable import canonical
from .portable_read import PortableArchive, rows

_OBJECT = TypeAdapter(dict[str, JsonValue])
_STRINGS = TypeAdapter(tuple[str, ...])


def metadata(value: JsonValue) -> dict[str, JsonValue]:
    if isinstance(value, str):
        value = json.loads(value)
    return _OBJECT.validate_python(value or {})


def timestamp(value: JsonValue) -> TimePoint | None:
    if value is None:
        return None
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
        raise DomainError("bundle_time", "Portable timestamps require finite Unix seconds")
    return TimePoint(coordinate=round(value * 1000))


def normalize(loaded: PortableArchive, data: bytes) -> SubjectBundle:
    """The actual archive and image bytes are retained; no task or file-success is invented."""
    manifest = loaded.manifest
    digest = sha256(data).hexdigest()

    def identity(key: str) -> str:
        return str(uuid5(NAMESPACE_URL, f"nous:portable:{digest}:{key}"))

    namespace_id, subject_id = identity("namespace"), identity("subject")
    created = manifest.get("created_at")
    if not isinstance(created, str):
        raise DomainError("bundle_time", "Portable archive must retain its actual creation time")
    moment = datetime.fromisoformat(created)
    if moment.tzinfo is None:
        raise DomainError("bundle_time", "Portable creation time requires its original offset")
    now = TimePoint(coordinate=round(moment.timestamp() * 1000))
    package = PackageDescriptor.model_validate(manifest.get("package"))
    materials: list[BundleMaterial] = []
    observations: list[BundleObservation] = []
    contents: dict[str, str] = {}

    def source(key: str, content: bytes, filename: str, mime: str) -> tuple[str, str]:
        material_id, observation_id = identity(f"material:{key}"), identity(f"observation:{key}")
        content_hash = sha256(content).hexdigest()
        material_record = _OBJECT.validate_python(
            {
                "id": material_id,
                "namespace_id": namespace_id,
                "content_hash": content_hash,
                "text": None,
                "filename": filename,
                "media_type": mime,
                "byte_len": len(content),
                "source_type": "amemorix_bundle",
                "origin": {"package_id": package.id},
                "event_time": None,
                "recorded_at": created,
                "commit_seq": 0,
            }
        )
        materials.append(
            BundleMaterial(
                id=material_id,
                namespace_id=namespace_id,
                content_hash=content_hash,
                record=material_record,
            )
        )
        observation_record = _OBJECT.validate_python(
            {
                "id": observation_id,
                "namespace_id": namespace_id,
                "subject_id": subject_id,
                "material_id": material_id,
                "kind": "import",
                "origin": {"package_id": package.id},
                "source_type": "amemorix_bundle",
                "event_time": None,
                "observed_at": None,
                "external_recorded_at": created,
                "recorded_at": created,
                "commit_seq": 0,
            }
        )
        observations.append(
            BundleObservation(
                id=observation_id,
                namespace_id=namespace_id,
                subject_id=subject_id,
                material_id=material_id,
                kind="import",
                active=True,
                record=observation_record,
                commit_seq=0,
            )
        )
        contents[content_hash] = base64.b64encode(content).decode("ascii")
        return material_id, observation_id

    _, archive_observation = source("archive", data, f"{package.id}.amembundle", "application/zip")
    evidence = (EvidenceLink(observation_id=archive_observation, role="portable_semantic_record"),)
    records: list[CognitiveRecord] = []

    def adopt(
        key: str,
        body: CognitiveBody,
        *,
        links: tuple[EvidenceLink, ...] = evidence,
        roots: tuple[str, ...] = (archive_observation,),
        raw: dict[str, JsonValue] | None = None,
    ) -> CognitiveRef:
        values = raw or {}
        if roots == (archive_observation,) and links != evidence:
            known = {record.reference.exact(): record for record in records}
            roots = tuple(
                sorted(
                    {
                        *[link.observation_id for link in links if link.observation_id],
                        *[
                            root
                            for link in links
                            if link.reference
                            for root in known[link.reference.exact()].source_roots
                        ],
                    }
                )
            )
        ref = CognitiveRef(kind=body.kind, id=identity(f"{body.kind}:{key}"), revision=1)
        records.append(
            CognitiveRecord(
                reference=ref,
                namespace_id=namespace_id,
                subject_id=subject_id,
                body=body,
                evidence=links,
                source_roots=roots,
                support_roots=roots,
                times=CognitiveTimes(
                    event=timestamp(values.get("event_time")),
                    observation=now,
                    formation=timestamp(values.get("created_at")) or now,
                    recorded=timestamp(values.get("updated_at")) or now,
                ),
                commit_seq=0,
                deleted=bool(values.get("is_deleted")),
            )
        )
        return ref

    tables = metadata(loaded.state.get("tables")) if loaded.state else {}
    paragraphs = rows(tables.get("paragraphs"))
    docs = rows(loaded.knowledge.get("docs"))
    if not paragraphs:
        paragraphs = rows(
            [
                {
                    "hash": doc.get("id"),
                    "content": doc.get("passage"),
                    "metadata": doc.get("a_memorix"),
                    "knowledge_doc": doc,
                }
                for doc in docs
            ]
        )
    memories: dict[str, CognitiveRef] = {}
    for paragraph in paragraphs:
        key, text = str(paragraph.get("hash") or ""), paragraph.get("content")
        if not key or not isinstance(text, str) or not text.strip() or key in memories:
            raise DomainError(
                "bundle_paragraph", "Paragraphs require unique identities and real text"
            )
        _, paragraph_observation = source(
            "paragraph:" + key, canonical(paragraph), key + ".json", "application/json"
        )
        source_label = paragraph.get("source") or metadata(paragraph.get("metadata")).get("source")
        if isinstance(source_label, str) and source_label:
            next(value for value in observations if value.id == paragraph_observation).record[
                "source"
            ] = source_label
            next(
                value for value in materials if value.id == identity("material:paragraph:" + key)
            ).record["source"] = source_label
        memories[key] = adopt(
            key,
            MemoryBody(
                text=text,
                external_ids={"amemorix:paragraph": key},
                metadata=_OBJECT.validate_python(
                    {
                        "portable_record": paragraph,
                        **metadata(paragraph.get("metadata")),
                        **({"source": source_label} if isinstance(source_label, str) else {}),
                    }
                ),
                epistemic=EpistemicStance(
                    classification="reported", attribution=f"package:{package.id}"
                ),
            ),
            links=(
                EvidenceLink(observation_id=paragraph_observation, role="portable_semantic_record"),
            ),
            roots=(paragraph_observation,),
            raw=paragraph,
        )
    entities: dict[str, CognitiveRef] = {}
    entity_names: dict[str, CognitiveRef] = {}
    from .portable_identity import entity_key, relation_key

    entity_rows = rows(tables.get("entities"))
    if not entity_rows:
        entity_rows = [
            {"hash": entity_key(str(name)), "name": name}
            for name in sorted(
                {
                    str(name)
                    for doc in docs
                    for name in _STRINGS.validate_python(doc.get("extracted_entities") or ())
                }
            )
        ]
    entity_links = rows(tables.get("paragraph_entities"))
    for entity in entity_rows:
        key, name = str(entity.get("hash") or ""), str(entity.get("name") or "")
        if not key or not name or key in entities:
            raise DomainError("bundle_entity", "Entities require unique identities and labels")
        mentioned = {
            str(link.get("paragraph_hash"))
            for link in entity_links
            if link.get("entity_hash") == key
        }
        mentioned.update(
            str(doc.get("id"))
            for doc in docs
            if name in _STRINGS.validate_python(doc.get("extracted_entities") or ())
        )
        refs = tuple(
            memories[paragraph] for paragraph in sorted(mentioned) if paragraph in memories
        )
        body_metadata = metadata(entity.get("metadata"))
        controls = PersonProfileControls.model_validate(body_metadata.get("profile_controls") or {})
        if controls.aliases is not None or controls.summary is not None:
            _, control_observation = source(
                "entity:" + key, canonical(entity), key + ".json", "application/json"
            )
            imported_evidence = (
                EvidenceLink(observation_id=control_observation, role="portable_profile_record"),
            )
            controls = controls.model_copy(
                update={
                    "aliases": controls.aliases.model_copy(update={"evidence": imported_evidence})
                    if controls.aliases is not None
                    else None,
                    "summary": controls.summary.model_copy(update={"evidence": imported_evidence})
                    if controls.summary is not None
                    else None,
                }
            )
        else:
            imported_evidence = ()
        ref = adopt(
            key,
            EntityBody.model_validate(
                {
                    "label": name,
                    "entity_type": body_metadata.get("entity_type", "other"),
                    "identifiers": {
                        **TypeAdapter(dict[str, str]).validate_python(
                            body_metadata.get("identifiers") or {}
                        ),
                        "amemorix:entity": key,
                    },
                    "aliases": _STRINGS.validate_python(body_metadata.get("aliases") or ()),
                    "description": TypeAdapter(str).validate_python(
                        body_metadata.get("description", entity.get("description") or "")
                    ),
                    "profile_controls": controls,
                    "mentions": tuple(EntityMention(reference=item) for item in refs),
                }
            ),
            links=(
                *(tuple(EvidenceLink(reference=item) for item in refs) or evidence),
                *imported_evidence,
            ),
        )
        entities[key], entity_names[name] = ref, ref
    relation_rows = rows(tables.get("relations"))
    if not relation_rows:
        for doc in docs:
            triples = doc.get("extracted_triples")
            if not isinstance(triples, list):
                continue
            for triple in triples:
                if not isinstance(triple, list) or len(triple) != 3:
                    raise DomainError("bundle_relation", "Extracted triples require three terms")
                relation_rows.append(
                    {
                        "hash": relation_key(str(triple[0]), str(triple[1]), str(triple[2])),
                        "subject": triple[0],
                        "predicate": triple[1],
                        "object": triple[2],
                        "paragraph_hash": doc.get("id"),
                    }
                )
    relation_links = rows(tables.get("paragraph_relations"))
    seen_relations: set[str] = set()
    for relation in relation_rows:
        key = str(relation.get("hash") or "")
        left = entity_names.get(str(relation.get("subject") or ""))
        right = entity_names.get(str(relation.get("object") or ""))
        predicate = str(relation.get("predicate") or "")
        if key in seen_relations:
            continue
        if not key or left is None or right is None or not predicate:
            raise DomainError(
                "bundle_relation", "Relations must bind actual unique Entity endpoints"
            )
        seen_relations.add(key)
        mentioned = {
            str(link.get("paragraph_hash"))
            for link in relation_links
            if link.get("relation_hash") == key
        }
        mentioned.update(
            str(row["paragraph_hash"])
            for row in relation_rows
            if row.get("hash") == key and row.get("paragraph_hash")
        )
        links = tuple(
            EvidenceLink(reference=memories[p]) for p in sorted(mentioned) if p in memories
        )
        relation_epistemic = metadata(relation.get("metadata")).get("epistemic")
        adopt(
            key,
            AssociationBody(
                from_ref=left,
                to_ref=right,
                relation=predicate,
                basis_class="source_evidence",
                weight=TypeAdapter(float | None).validate_python(
                    metadata(relation.get("metadata")).get("weight")
                ),
                epistemic=EpistemicStance.model_validate(relation_epistemic)
                if relation_epistemic is not None
                else None,
            ),
            links=links or evidence,
            raw=relation,
        )
    episode_links = rows(tables.get("episode_paragraphs"))
    for episode in rows(tables.get("episodes")):
        key = str(episode.get("episode_id") or "")
        linked = sorted(
            [link for link in episode_links if link.get("episode_id") == key],
            key=lambda row: int(str(row.get("position") or 0)),
        )
        members = tuple(
            EpisodeMember(reference=memories[str(row["paragraph_hash"])], ordinal=index)
            for index, row in enumerate(linked)
            if str(row.get("paragraph_hash")) in memories
        )
        start, end = (
            timestamp(episode.get("event_time_start")),
            timestamp(episode.get("event_time_end")),
        )
        adopt(
            key,
            EpisodeBody(
                title=str(episode.get("title") or key),
                track=str(episode.get("source") or "portable"),
                experience=TimeRange(
                    start=start.coordinate if start else None, end=end.coordinate if end else None
                ),
                members=members,
                summary=str(episode.get("summary") or ""),
                boundary_explanation="Imported A-Memorix episode membership",
            ),
            links=tuple(EvidenceLink(reference=member.reference) for member in members) or evidence,
            raw=episode,
        )
    from .portable_fact_adoption import adopt_facts

    adopt_facts(tables, records, entities, memories, source, adopt)
    from .portable_image_adoption import adopt_images

    derivations = adopt_images(
        loaded,
        identity,
        source,
        records,
        observations,
        materials,
        namespace_id,
        subject_id,
        now,
        created,
    )
    from .portable_vectors import import_vectors

    vector_targets = {key: ref.exact() for key, ref in memories.items()}
    vector_targets.update({"entity:" + key: ref.exact() for key, ref in entities.items()})
    vector_targets.update(
        {
            "relation:" + key: f"association:{identity('association:' + key)}@1"
            for key in seen_relations
        }
    )
    image_targets: dict[str, list[str]] = {}
    if loaded.images is not None:
        for occurrence in rows(loaded.images.get("occurrences")):
            asset_id = str(occurrence["asset_id"])
            image_targets.setdefault(asset_id, []).append(
                "material:" + identity("material:" + str(occurrence["occurrence_id"]))
            )
    vector_assets = import_vectors(loaded, vector_targets, image_targets)
    return SubjectBundle(
        package=package,
        content_level="full" if loaded.manifest["content_level"] == "full" else "knowledge",
        id=identity("bundle"),
        namespace_id=namespace_id,
        subject_id=subject_id,
        authority_commit_seq=0,
        cognition=tuple(records),
        materials=tuple(materials),
        observations=tuple(observations),
        contents=contents,
        derivations=derivations,
        vectors=vector_assets,
    )
