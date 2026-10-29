"""Validate published A-Memorix archives before interpreting any semantic rows.

Member limits, checksum coverage and content digest follow the upstream
bundle_admin_service loader. JSON identities remain data until adoption.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from hashlib import sha256
from io import BytesIO
from pathlib import PurePosixPath
from zipfile import BadZipFile, ZipFile

from nous_wave.contracts import DomainError
from pydantic import JsonValue, TypeAdapter

from .portable import canonical

_OBJECT = TypeAdapter(dict[str, JsonValue])
_ROWS = TypeAdapter(list[dict[str, JsonValue]])


def rows(value: JsonValue) -> list[dict[str, JsonValue]]:
    return _ROWS.validate_python(value or [])


def evidence(value: JsonValue) -> dict[str, JsonValue]:
    if isinstance(value, str):
        value = json.loads(value)
    if not isinstance(value, dict):
        return {}
    ephemeral = {"chat_id", "external_ref", "imported", "message_id", "source_link_id"}
    return {key: item for key, item in value.items() if key not in ephemeral}


def image_semantics(images: dict[str, JsonValue]) -> dict[str, JsonValue]:
    assets = {str(row.get("asset_id") or ""): row for row in rows(images.get("assets"))}
    occurrences = sorted(
        rows(images.get("occurrences")),
        key=lambda row: (
            str(assets.get(str(row.get("asset_id") or ""), {}).get("content_hash") or ""),
            str(row.get("component_path") or ""),
            str(row.get("occurrence_id") or ""),
        ),
    )
    keys = {
        str(row.get("occurrence_id") or ""): f"occurrence-{index}"
        for index, row in enumerate(occurrences)
    }
    canonical_assets = [
        {
            "byte_size": int(str(row.get("byte_size") or 0)),
            "content_hash": str(row.get("content_hash") or ""),
            "height": int(str(row.get("height") or 0)),
            "mime_type": str(row.get("mime_type") or ""),
            "width": int(str(row.get("width") or 0)),
        }
        for row in assets.values()
    ]
    observations = [
        {
            "confirm_status": str(row.get("confirm_status") or ""),
            "occurrence": keys.get(str(row.get("occurrence_id") or ""), ""),
            "source_kind": str(row.get("source_kind") or ""),
            "text": str(row.get("text") or ""),
            "evidence": evidence(row.get("evidence_json")),
        }
        for row in rows(images.get("observations"))
    ]
    links = [
        {
            "link_kind": str(row.get("link_kind") or ""),
            "occurrence": keys.get(str(row.get("occurrence_id") or ""), ""),
            "target_id": str(row.get("target_id") or ""),
            "target_type": str(row.get("target_type") or ""),
            "evidence": evidence(row.get("evidence_json")),
        }
        for row in rows(images.get("links"))
    ]
    return _OBJECT.validate_python(
        {
            "assets": sorted(canonical_assets, key=lambda row: row["content_hash"]),
            "occurrences": [
                {
                    "asset_hash": str(
                        assets.get(str(row.get("asset_id") or ""), {}).get("content_hash") or ""
                    ),
                    "component_path": str(row.get("component_path") or ""),
                }
                for row in occurrences
            ],
            "observations": sorted(
                observations, key=lambda row: (row["occurrence"], row["source_kind"], row["text"])
            ),
            "links": sorted(
                links, key=lambda row: (row["occurrence"], row["target_type"], row["target_id"])
            ),
        }
    )


@dataclass(frozen=True)
class PortableArchive:
    manifest: dict[str, JsonValue]
    knowledge: dict[str, JsonValue]
    state: dict[str, JsonValue] | None
    images: dict[str, JsonValue] | None
    members: dict[str, bytes]


def load(data: bytes) -> PortableArchive:
    try:
        with ZipFile(BytesIO(data)) as archive:
            infos = archive.infolist()
            names = {info.filename for info in infos}
            if (
                len(infos) > 4096
                or len(names) != len(infos)
                or any(info.file_size > 256 * 1024 * 1024 for info in infos)
                or sum(info.file_size for info in infos) > 1024 * 1024 * 1024
            ):
                raise DomainError("bundle_limits", "Archive exceeds member or expanded size limits")
            if any(
                not name
                or "\\" in name
                or PurePosixPath(name).is_absolute()
                or ".." in PurePosixPath(name).parts
                for name in names
            ):
                raise DomainError("bundle_path", "Archive contains an invalid member path")
            if not {"manifest.json", "knowledge.json", "checksums.json"}.issubset(names):
                raise DomainError(
                    "bundle_members", "Archive is missing its required semantic members"
                )
            members = {info.filename: archive.read(info) for info in infos}
        manifest = _OBJECT.validate_json(members["manifest.json"])
        knowledge = _OBJECT.validate_json(members["knowledge.json"])
        checksums = _OBJECT.validate_json(members["checksums.json"])
        state = _OBJECT.validate_json(members["state.json"]) if "state.json" in members else None
        images = _OBJECT.validate_json(members["images.json"]) if "images.json" in members else None
        version = manifest.get("format_version")
        if manifest.get("format") != "a-memorix-memory-bundle" or version not in {1, 2}:
            raise DomainError("bundle_format", "Unsupported A-Memorix archive format")
        if (version == 1 and images is not None) or (version == 2 and images is None):
            raise DomainError("bundle_images", "Image extension does not match archive version")
        if not isinstance(knowledge.get("docs"), list):
            raise DomainError("bundle_knowledge", "Knowledge must contain a docs array")
        level = manifest.get("content_level")
        if level not in {"knowledge", "full"} or (level == "full" and state is None):
            raise DomainError("bundle_state", "Full archives require their semantic state")
        hashes = checksums.get("files")
        if checksums.get("algorithm") != "sha256" or not isinstance(hashes, dict):
            raise DomainError("bundle_checksums", "Archive requires SHA-256 checksums")
        if set(hashes) != names - {"checksums.json"} or any(
            sha256(members[name]).hexdigest() != digest for name, digest in hashes.items()
        ):
            raise DomainError("bundle_checksums", "Archive checksum coverage or content is invalid")
        if images is not None:
            declared: set[str] = set()
            for asset in rows(images.get("assets")):
                member = str(asset.get("member") or "")
                if (
                    member in declared
                    or not member.startswith("images/assets/")
                    or member not in members
                    or hashes.get(member) != asset.get("content_hash")
                    or len(members[member]) != asset.get("byte_size")
                ):
                    raise DomainError(
                        "bundle_image_asset", "Image bytes differ from their asset identity"
                    )
                declared.add(member)
            if declared != {name for name in names if name.startswith("images/assets/")}:
                raise DomainError("bundle_image_asset", "Archive contains undeclared image assets")
        content: dict[str, JsonValue] = {
            "knowledge": knowledge,
            "state": state if level == "full" else None,
        }
        if images is not None:
            content["images"] = image_semantics(images)
        if sha256(canonical(content)).hexdigest() != manifest.get("content_digest"):
            raise DomainError("bundle_content_digest", "Archive semantic digest does not match")
        return PortableArchive(manifest, knowledge, state, images, members)
    except (BadZipFile, KeyError, UnicodeError, ValueError, TypeError) as error:
        raise DomainError(
            "bundle_format", "Archive members or semantic rows are invalid"
        ) from error
