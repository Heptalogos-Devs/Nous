"""Recognize neutral default additions when replaying stored public commands."""

from typing import cast

from pydantic import BaseModel, JsonValue, TypeAdapter

_JSON = TypeAdapter(JsonValue)
_ENCODE = TypeAdapter(object)
_ADDED_DEFAULTS: dict[str, tuple[str, ...]] = {
    "ProcessCatalog": (
        "formation_options_override",
        "deferred_formation",
        "origin",
        "retry_parent_job_id",
    ),
    "AdmitFiles": ("deferred_formation",),
    "FreezeCatalog": ("record_units",),
    "TextSplitOptions": ("record_units",),
    "FormObservation": ("anchor", "vectors"),
    "ImportDocument": ("source",),
    "BlobDocument": ("source",),
    "SourceCommand": ("requested_by",),
    "AdoptMedia": ("observation_id",),
    "OpenContextSession": ("parent_session_id", "history_generation"),
    "MemoryBody": ("media",),
    "EntityBody": ("purged_attribute_count", "purged_identity_count", "platform_identities"),
    "SocialRelationBody": ("dimension", "groups", "epistemic"),
    "FormationCatalogEntry": ("material_ids",),
    "InstallBundle": ("mode", "placement", "archive_material_id"),
}


def previous_payload(value: object) -> JsonValue:
    """Only known default-valued additions may disappear; changed values retain their identity."""
    encoded = _JSON.validate_python(_ENCODE.dump_python(value, mode="json"))
    if isinstance(value, BaseModel) and isinstance(encoded, dict):
        cls = type(value)
        for name in cls.model_fields:
            if name in encoded:
                encoded[name] = previous_payload(getattr(value, name))
        if cls.__module__.startswith("nous_wave.contracts."):
            for name in _ADDED_DEFAULTS.get(cls.__name__, ()):
                field = cls.model_fields[name]
                default = _JSON.validate_python(
                    _ENCODE.dump_python(field.get_default(call_default_factory=True), mode="json")
                )
                if encoded.get(name) == default:
                    encoded.pop(name, None)
        return encoded
    if isinstance(value, (list, tuple)):
        return [previous_payload(child) for child in cast(list[object] | tuple[object, ...], value)]
    if isinstance(value, dict):
        return {
            key: previous_payload(child) for key, child in cast(dict[str, object], value).items()
        }
    return encoded
