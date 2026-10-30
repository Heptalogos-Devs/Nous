"""Decode the established A-Memorix payload into the shared Bundle contract."""

from __future__ import annotations

from nous_sdk.contracts import DomainError
from pydantic import JsonValue


def decode_bundle_arguments(arguments: dict[str, JsonValue]) -> dict[str, JsonValue]:
    value = dict(arguments)
    selector = value.get("selector")
    if isinstance(selector, dict) and "type" in selector:
        kind = selector["type"]
        token = selector.get("value")
        if kind == "all":
            selected: dict[str, JsonValue] = {"kind": "all"}
        elif kind in {"image", "images"}:
            selected = {
                "kind": "images",
                "content_hashes": selector.get("content_hashes") or selector.get("values") or [],
                "occurrence_ids": selector.get("occurrence_ids") or [],
            }
        elif kind == "source":
            selected = {
                "kind": "source",
                "sources": selector.get("sources")
                or selector.get("values")
                or ([token] if isinstance(token, str) else []),
            }
        elif kind == "chat":
            selected = {"kind": "chat", "conversation_ref": selector.get("chat_id") or token}
        elif kind == "package":
            selected = {
                "kind": "package_token",
                "token": selector.get("package_id") or selector.get("installation_id") or token,
            }
        elif kind == "import_task":
            selected = {
                "kind": "import_task",
                "job_id": selector.get("task_id") or selector.get("job_id") or token,
            }
        else:
            raise DomainError("bundle_selector", "Unsupported A-Memorix bundle selector")
        value["selector"] = selected
    assets = value.get("assets")
    selected_assets = dict(assets) if isinstance(assets, dict) else {}
    for field in ("include_vectors", "include_images", "include_image_related"):
        if field in value:
            selected_assets[field] = value.pop(field)
    if selected_assets:
        value["assets"] = selected_assets
    package = value.get("package")
    if isinstance(package, dict) and "package_id" in package:
        value["package"] = {
            "id" if key == "package_id" else key: item for key, item in package.items()
        }
    value.setdefault("format", "amemorix")
    if value["format"] == "amemorix":
        value.setdefault("content_level", "knowledge")
        selected_assets.setdefault("include_vectors", True)
        value["assets"] = selected_assets
    if value.get("action") == "preview":
        value["action"], value["preview"] = "export", True
    return value
