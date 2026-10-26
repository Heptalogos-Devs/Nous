from collections.abc import Mapping
from uuid import UUID


def target_reference(value: str | Mapping) -> dict:
    if isinstance(value, Mapping):
        return dict(value)
    kind, identity = value.split(":", 1)
    if kind in {"material", "source-region", "derived-region"}:
        identity = str(UUID(identity))
        key, field = {
            "material": ("original", "material_id"),
            "source-region": ("source_region", "id"),
            "derived-region": ("derived_region", "id"),
        }[kind]
        return {"kind": "media", "reference": {"kind": key, field: identity}}
    identifier, revision = identity.rsplit("@", 1)
    revision = int(revision)
    if revision <= 0:
        raise ValueError("source requires a positive exact revision")
    identifier = str(UUID(identifier))
    if kind == "representation":
        return {
            "kind": "media",
            "reference": {
                "kind": "representation",
                "reference": {"id": identifier, "revision": revision},
            },
        }
    return {
        "kind": "cognition",
        "reference": {"kind": kind, "id": identifier, "revision": revision},
    }
