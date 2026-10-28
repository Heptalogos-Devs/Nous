from collections.abc import Mapping

from pydantic import JsonValue, TypeAdapter

from ..contracts.context import ContextTarget


def target_reference(value: str | Mapping[str, JsonValue] | ContextTarget) -> ContextTarget:
    if not isinstance(value, (str, Mapping)):
        return value
    if isinstance(value, Mapping):
        return TypeAdapter(ContextTarget).validate_python(value)
    kind, identity = value.split(":", 1)
    if kind in {"material", "source-region", "source_region", "derived-region", "derived_region"}:
        return TypeAdapter(ContextTarget).validate_python(
            {"kind": "media", "reference": {"kind": kind.replace("-", "_"), "id": identity}}
        )
    identifier, version = identity.rsplit("@", 1)
    return TypeAdapter(ContextTarget).validate_python(
        {
            "kind": "cognition",
            "reference": {"kind": kind, "id": identifier, "revision": int(version)},
        }
    )
