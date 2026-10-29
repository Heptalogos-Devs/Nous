"""Published NPZ assets keep their exact declared embedding-space fingerprint."""

from __future__ import annotations

from hashlib import sha256
from io import BytesIO
from zipfile import BadZipFile, ZipFile

import numpy as np
from nous_wave.contracts import BundleRequest, DomainError, SubjectBundle
from nous_wave.contracts.management import BundleVector
from numpy.typing import NDArray
from pydantic import JsonValue

from .portable_read import PortableArchive, rows


def _coordinates(data: bytes) -> tuple[list[str], NDArray[np.float64]]:
    with ZipFile(BytesIO(data)) as zipped:
        if (
            len(zipped.infolist()) != 2
            or {item.filename for item in zipped.infolist()} != {"ids.npy", "vectors.npy"}
            or sum(item.file_size for item in zipped.infolist()) > 256 * 1024 * 1024
        ):
            raise DomainError(
                "bundle_vector_limits", "Vector archive must contain bounded ids and coordinates"
            )
    with np.load(BytesIO(data), allow_pickle=False) as archive:
        ids, values = archive["ids"], archive["vectors"]
        if (
            ids.ndim != 1
            or ids.dtype.kind not in {"U", "S"}
            or values.ndim != 2
            or values.dtype.kind not in {"f", "i", "u"}
            or len(ids) != len(values)
            or not values.shape[1]
            or not np.isfinite(values).all()
        ):
            raise DomainError(
                "bundle_vector_shape", "Vector IDs and finite coordinate rows must match"
            )
        names = [
            value.decode("utf-8") if isinstance(value, bytes) else str(value)
            for value in ids.tolist()
        ]
        if len(set(names)) != len(names) or any(not value for value in names):
            raise DomainError("bundle_vector_ids", "Vector identities must be unique and nonempty")
        return names, np.asarray(values, dtype=np.float64)


def coordinates(data: bytes) -> tuple[list[str], NDArray[np.float64]]:
    try:
        return _coordinates(data)
    except (BadZipFile, ValueError, KeyError, UnicodeError) as error:
        raise DomainError(
            "bundle_vector_format", "Vector archive has invalid identities or coordinate arrays"
        ) from error


def import_vectors(
    loaded: PortableArchive, targets: dict[str, str], image_targets: dict[str, list[str]]
) -> tuple[BundleVector, ...]:
    from .portable import canonical

    result: list[BundleVector] = []
    for member, field in (
        ("vectors/paragraphs.npz", "embedding_fingerprint"),
        ("vectors/graph.npz", "embedding_fingerprint"),
        ("vectors/images.npz", "image_embedding_fingerprint"),
    ):
        data = loaded.members.get(member)
        if data is None:
            continue
        fingerprint = loaded.manifest.get(field)
        if (
            not isinstance(fingerprint, dict)
            or not isinstance(fingerprint.get("hash"), str)
            or not isinstance(fingerprint.get("model"), str)
            or fingerprint["model"] in {"", "auto"}
        ):
            raise DomainError(
                "bundle_vector_fingerprint",
                "Vector assets require their actual model and complete declared fingerprint",
            )
        names, values = coordinates(data)
        if (
            type(fingerprint.get("dimension")) is not int
            or fingerprint.get("dimension") != values.shape[1]
        ):
            raise DomainError(
                "bundle_vector_space",
                "Vector dimension differs from its actual declared fingerprint",
            )
        native_space = fingerprint.get("nous_space")
        space = (
            native_space
            if isinstance(native_space, str) and native_space
            else "amemorix:" + sha256(canonical(fingerprint)).hexdigest()
        )
        for key, vector in zip(names, values, strict=True):
            selected = (
                image_targets.get(key, [])
                if member.endswith("images.npz")
                else [targets[key]]
                if key in targets
                else []
            )
            if not selected:
                raise DomainError(
                    "bundle_vector_target",
                    "Vector target is outside the semantic asset closure",
                    reference=key,
                )
            for target in selected:
                result.append(
                    BundleVector(
                        reference=target,
                        space=space,
                        model=str(fingerprint["model"]),
                        dimensions=values.shape[1],
                        vector=tuple(float(value) for value in vector),
                        commit_seq=0,
                        provenance={
                            "format": "a-memorix-memory-bundle",
                            "fingerprint": fingerprint,
                            "member": member,
                            "external_id": key,
                        },
                    )
                )
    return tuple(result)


def export_vectors(
    bundle: SubjectBundle,
    command: BundleRequest,
    images: dict[str, JsonValue] | None,
    portable_ids: set[str],
) -> tuple[dict[str, bytes], dict[str, JsonValue] | None, dict[str, JsonValue] | None]:

    if command.assets is None or not command.assets.include_vectors:
        return {}, None, None
    from .portable_identity import semantic_ids

    identities = semantic_ids(bundle)
    candidates = [
        row
        for row in bundle.vectors
        if not command.assets.vector_spaces or row.space in command.assets.vector_spaces
    ]
    groups: dict[str, list[tuple[str, BundleVector]]] = {
        "paragraphs": [],
        "graph": [],
        "images": [],
    }
    record_ids = {
        value.reference.exact(): value.reference
        for value in bundle.cognition
        if not value.deleted and not value.purged
    }
    image_ids: set[str] = (
        {str(row["content_hash"]) for row in rows(images.get("assets"))} if images else set()
    )
    materials = {"material:" + row.id: row.content_hash for row in bundle.materials}
    for value in candidates:
        ref = record_ids.get(value.reference)
        if (
            ref
            and identities.get(ref.id) in portable_ids
            and ref.kind in {"memory", "entity", "association"}
        ):
            group = "paragraphs" if ref.kind == "memory" else "graph"
            key = (
                identities[ref.id]
                if ref.kind == "memory"
                else ("entity:" if ref.kind == "entity" else "relation:") + identities[ref.id]
            )
            groups[group].append((key, value))
        elif materials.get(value.reference) in image_ids:
            groups["images"].append((materials[value.reference], value))
    fingerprints: dict[str, dict[str, JsonValue]] = {}
    members: dict[str, bytes] = {}
    for group, items in groups.items():
        if not items:
            continue
        field = "image_embedding_fingerprint" if group == "images" else "embedding_fingerprint"
        spaces = {row.space for _, row in items}
        if len(spaces) != 1:
            raise DomainError(
                "bundle_vector_spaces",
                "Portable pools require one exact embedding space; select vector_spaces",
                pool=group,
                spaces=sorted(spaces),
            )
        first = items[0][1]
        declared = first.provenance.get("fingerprint")
        fingerprint: dict[str, JsonValue] = (
            declared
            if isinstance(declared, dict)
            else {
                "version": 1,
                "model": first.model,
                "dimension": first.dimensions,
                "hash": first.space,
                "nous_space": first.space,
            }
        )
        if field in fingerprints and fingerprints[field] != fingerprint:
            raise DomainError(
                "bundle_vector_spaces",
                "Paragraph and graph vectors must share an exact declared space",
            )
        fingerprints[field] = fingerprint
        unique: dict[str, tuple[float, ...]] = {}
        for key, row in items:
            if (
                row.model != first.model
                or row.dimensions != first.dimensions
                or (key in unique and unique[key] != row.vector)
            ):
                raise DomainError(
                    "bundle_vector_space", "A portable pool mixes inconsistent assets"
                )
            unique[key] = row.vector
        output = BytesIO()
        np.savez_compressed(
            output,
            ids=np.asarray(list(unique)),
            vectors=np.asarray(list(unique.values()), dtype=np.float32),
        )
        members[f"vectors/{group}.npz"] = output.getvalue()
    return (
        members,
        fingerprints.get("embedding_fingerprint"),
        fingerprints.get("image_embedding_fingerprint"),
    )
