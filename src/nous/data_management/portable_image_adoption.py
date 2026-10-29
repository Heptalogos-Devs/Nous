"""Adopt published image descriptions and exact semantic links as source facts."""

from __future__ import annotations

from collections.abc import Callable
from hashlib import sha256

from nous_wave.contracts import (
    CognitiveRecord,
    DomainError,
    EvidenceLink,
    MediaReference,
    MemoryBody,
    TimePoint,
)
from nous_wave.contracts.derivation import (
    ChannelAccess,
    DerivationInput,
    DerivationProducer,
    DerivationQuality,
    DerivedBundle,
    DerivedRepresentation,
    RepresentationRef,
    TextRepresentation,
)
from nous_wave.contracts.management import BundleMaterial, BundleObservation
from pydantic import JsonValue, TypeAdapter

from .portable import canonical
from .portable_read import PortableArchive, rows


def adopt_images(
    loaded: PortableArchive,
    identity: Callable[[str], str],
    source: Callable[[str, bytes, str, str], tuple[str, str]],
    records: list[CognitiveRecord],
    events: list[BundleObservation],
    materials: list[BundleMaterial],
    namespace: str,
    subject: str,
    now: TimePoint,
    recorded: str,
) -> DerivedBundle:
    if loaded.images is None:
        return DerivedBundle()
    assets = {str(row["asset_id"]): row for row in rows(loaded.images.get("assets"))}
    descriptions = rows(loaded.images.get("observations"))
    links = rows(loaded.images.get("links"))
    representations: list[DerivedRepresentation] = []
    kinds = {
        "paragraph": "memory",
        "entity": "entity",
        "relation": "association",
        "episode": "episode",
    }
    for occurrence in rows(loaded.images.get("occurrences")):
        key = str(occurrence["occurrence_id"])
        asset = assets[str(occurrence["asset_id"])]
        material_id, event_id = source(
            key,
            loaded.members[str(asset["member"])],
            str(occurrence.get("component_path") or key),
            str(asset["mime_type"]),
        )
        material = next(row for row in materials if row.id == material_id)
        material.record["media_info"] = {
            "kind": "image",
            "media_type": str(asset["mime_type"]),
            "width": asset.get("width"),
            "height": asset.get("height"),
        }
        selected_descriptions = [row for row in descriptions if row.get("occurrence_id") == key]
        retained_links: list[dict[str, JsonValue]] = []
        for link in links:
            if link.get("occurrence_id") != key:
                continue
            kind = kinds.get(str(link.get("target_type")))
            target = str(link.get("target_id") or "")
            expected = identity(f"{kind}:{target}") if kind else None
            index = next(
                (i for i, value in enumerate(records) if value.reference.id == expected), None
            )
            if index is None:
                raise DomainError(
                    "bundle_image_link", "Image link must identify an included semantic object"
                )
            value = records[index]
            body = value.body
            if isinstance(body, MemoryBody):
                body = body.model_copy(
                    update={
                        "media": (
                            *body.media,
                            MediaReference(
                                kind="material", id=material_id, observation_id=event_id
                            ),
                        )
                    }
                )
            records[index] = value.model_copy(
                update={
                    "body": body,
                    "evidence": (
                        *value.evidence,
                        EvidenceLink(
                            observation_id=event_id,
                            role="image:" + str(link.get("link_kind") or "related"),
                            polarity="support" if link.get("link_kind") == "source" else "boundary",
                        ),
                    ),
                    "source_roots": tuple(sorted(set(value.source_roots) | {event_id})),
                    "support_roots": tuple(
                        sorted(
                            set(value.support_roots)
                            | ({event_id} if link.get("link_kind") == "source" else set())
                        )
                    ),
                }
            )
            retained_links.append(
                {**link, "target_reference": value.reference.model_dump(mode="json")}
            )
        event = next(row for row in events if row.id == event_id)
        event.record["amemorix_image"] = TypeAdapter(dict[str, JsonValue]).validate_python(
            {
                "occurrence": occurrence,
                "asset": {name: value for name, value in asset.items() if name != "member"},
                "observations": selected_descriptions,
                "links": retained_links,
            }
        )
        for index, description in enumerate(selected_descriptions):
            text = description.get("text")
            if not isinstance(text, str) or not text.strip():
                raise DomainError(
                    "bundle_image_description",
                    "Image observation requires its actual nonempty text",
                )
            signature = sha256(canonical(description)).hexdigest()
            representations.append(
                DerivedRepresentation(
                    reference=RepresentationRef(
                        id=identity(f"image-description:{key}:{index}"), revision=1
                    ),
                    namespace_id=namespace,
                    subject_id=subject,
                    inputs=(
                        DerivationInput(
                            ordinal=0,
                            source=MediaReference(
                                kind="material", id=material_id, observation_id=event_id
                            ),
                        ),
                    ),
                    kind="image_description",
                    strategy="description_only",
                    payload=TextRepresentation(text=text),
                    producer=DerivationProducer(
                        signature=signature,
                        implementation="amemorix:imported-image-observation",
                        config_digest=sha256(canonical({})).hexdigest(),
                    ),
                    access=ChannelAccess(visual="original"),
                    quality=DerivationQuality(),
                    source_roots=(event_id,),
                    material_ids=(material_id,),
                    formed_at=now,
                    recorded_at=recorded,
                    commit_seq=0,
                )
            )
    return DerivedBundle(representations=tuple(representations))
