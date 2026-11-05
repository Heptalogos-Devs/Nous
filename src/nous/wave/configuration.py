"""Wave's application-level options; numerical profiles belong to Native."""

import copy
import math
import os
from collections.abc import Callable
from functools import lru_cache
from importlib.util import find_spec
from pathlib import Path
from typing import Literal, cast

from jsonschema import Draft202012Validator
from jsonschema import ValidationError as SchemaValidationError
from pydantic import Field, JsonValue, TypeAdapter, ValidationInfo, field_validator

from nous.configuration.metadata import ConfigModel, setting


@lru_cache(maxsize=1)
def _native_profiles() -> tuple[dict[str, JsonValue], dict[str, JsonValue]]:
    """Read packaged Rust-generated resources without loading the extension."""
    try:
        package = find_spec("nous_native")
        if package is None or not package.submodule_search_locations:
            return {}, {}
        root = Path(next(iter(package.submodule_search_locations))) / "resources"
        defaults = TypeAdapter(dict[str, JsonValue]).validate_json(
            root.joinpath("defaults.json").read_text(encoding="utf-8")
        )
        schemas = TypeAdapter(dict[str, JsonValue]).validate_json(
            root.joinpath("schemas.json").read_text(encoding="utf-8")
        )
    except ModuleNotFoundError, FileNotFoundError:
        return {}, {}
    return defaults, schemas


def require_native_profiles() -> None:
    defaults, schemas = _native_profiles()
    required = {"build", "pipeline", "readout", "dtsc", "query", "context"}
    if required - defaults.keys() or required - schemas.keys():
        from nous.common.errors import NousError

        raise NousError(
            "wave.native_profiles_unavailable",
            "Wave Native configuration resources are missing; install its generated defaults and schemas.",
        )


def _defaults(names: tuple[str, ...]) -> dict[str, JsonValue]:
    defaults, _ = _native_profiles()
    return copy.deepcopy({name: defaults[name] for name in names if name in defaults})


def _build_defaults() -> dict[str, JsonValue]:
    defaults, _ = _native_profiles()
    value = defaults.get("build", {})
    return copy.deepcopy(value) if isinstance(value, dict) else {}


def _profile_defaults(name: str) -> dict[str, JsonValue]:
    defaults, _ = _native_profiles()
    profile = defaults.get(name)
    if not isinstance(profile, dict):
        raise ValueError(f"Native defaults are unavailable: {name}")
    return copy.deepcopy(profile)


def _query_defaults() -> dict[str, JsonValue]:
    controls = _profile_defaults("query")
    controls.pop("rrf_k")
    controls.pop("top_k")
    controls.pop("group_scan_limit")
    return {**_defaults(("pipeline", "readout", "dtsc")), "controls": controls}


def _controls_schema() -> dict[str, JsonValue]:
    _, schemas = _native_profiles()
    raw = schemas.get("query")
    if not isinstance(raw, dict):
        raise ValueError("Native query schema is unavailable")
    schema = copy.deepcopy(raw)
    properties = cast(dict[str, JsonValue], schema["properties"])
    properties.pop("rrf_k")
    properties.pop("top_k")
    properties.pop("group_scan_limit")
    if isinstance(schema.get("required"), list):
        schema["required"] = [
            item
            for item in cast(list[JsonValue], schema["required"])
            if item not in {"rrf_k", "top_k", "group_scan_limit"}
        ]
    if isinstance(schema.get("default"), dict):
        cast(dict[str, JsonValue], schema["default"]).pop("rrf_k", None)
        cast(dict[str, JsonValue], schema["default"]).pop("top_k", None)
        cast(dict[str, JsonValue], schema["default"]).pop("group_scan_limit", None)
    return schema


def _expand(value: JsonValue, definitions: dict[str, JsonValue]) -> JsonValue:
    if isinstance(value, list):
        return [_expand(item, definitions) for item in value]
    if not isinstance(value, dict):
        return value
    result = dict(value)
    reference = result.pop("$ref", None)
    if isinstance(reference, str) and reference.startswith("#/$defs/"):
        result = {
            **dict(cast(dict[str, JsonValue], definitions[reference.removeprefix("#/$defs/")])),
            **result,
        }
    return {
        key: _expand(item, definitions)
        for key, item in result.items()
        if key not in {"$defs", "$schema"}
    }


def _property_schema(profile: str, name: str) -> dict[str, JsonValue]:
    _, schemas = _native_profiles()
    raw = schemas.get(profile)
    if not isinstance(raw, dict):
        raise ValueError(f"Native schema is unavailable: {profile}")
    properties = cast(dict[str, JsonValue], raw["properties"])
    return cast(
        dict[str, JsonValue],
        _expand(properties[name], cast(dict[str, JsonValue], raw.get("$defs", {}))),
    )


def _native_field_schema(profile: str, name: str, node: dict[str, JsonValue]) -> None:
    node.update(_property_schema(profile, name))
    node["x-nous"] = {
        "exposure": "advanced",
        "apply_mode": "next_operation",
        "impact": "ordinary",
        "scopes": ["instance"],
        "readonly": False,
        "secret_value": False,
    }


def _validate_native_field(profile: str, name: str, value: object) -> None:
    validate = cast(
        Callable[[object], None], Draft202012Validator(_property_schema(profile, name)).validate
    )
    try:
        validate(value)
    except SchemaValidationError as error:
        raise ValueError(f"invalid Native field {profile}.{name}: {error.message}") from error


def _schema(names: tuple[str, ...], node: dict[str, JsonValue]) -> None:
    _, schemas = _native_profiles()
    properties: dict[str, JsonValue] = {}
    for name in names:
        raw = schemas.get(name)
        if isinstance(raw, dict):
            properties[name] = _expand(raw, cast(dict[str, JsonValue], raw.get("$defs", {})))
    if properties:
        node["properties"] = properties
        node["additionalProperties"] = False


def _query_schema(node: dict[str, JsonValue]) -> None:
    _schema(("pipeline", "readout", "dtsc"), node)
    cast(dict[str, JsonValue], node["properties"])["controls"] = _controls_schema()
    node["x-nous"] = {
        "exposure": "advanced",
        "apply_mode": "next_operation",
        "impact": "ordinary",
        "scopes": ["instance"],
        "readonly": False,
        "secret_value": False,
    }


def _build_schema(node: dict[str, JsonValue]) -> None:
    _, schemas = _native_profiles()
    raw = schemas.get("build")
    if isinstance(raw, dict):
        expanded = _expand(raw, cast(dict[str, JsonValue], raw.get("$defs", {})))
        if isinstance(expanded, dict):
            node.update(expanded)
    node["x-nous"] = {
        "exposure": "advanced",
        "apply_mode": "new_binding",
        "impact": "ordinary",
        "scopes": ["instance"],
        "readonly": False,
        "secret_value": False,
    }


class NativeConfig(ConfigModel):
    threads: int = setting(
        min(4, os.process_cpu_count() or 1),
        ge=1,
        apply="restart",
        description="Native worker limit.",
    )
    build: dict[str, JsonValue] = Field(
        default_factory=_build_defaults,
        description="Native construction profile.",
        json_schema_extra=_build_schema,
    )
    query: dict[str, JsonValue] = Field(
        default_factory=_query_defaults,
        description="Native query profile.",
        json_schema_extra=_query_schema,
    )

    @field_validator("build", "query")
    @classmethod
    def validate_native_profile(
        cls, value: dict[str, JsonValue], info: ValidationInfo
    ) -> dict[str, JsonValue]:
        _, schemas = _native_profiles()
        profiles = {"build": value} if info.field_name == "build" else value
        for name, options in profiles.items():
            raw = (
                _controls_schema()
                if info.field_name == "query" and name == "controls"
                else schemas.get(name)
            )
            if not isinstance(raw, dict):
                if not options:
                    continue
                raise ValueError(f"unknown or unavailable Native profile: {name}")
            validate = cast(Callable[[object], None], Draft202012Validator(raw).validate)
            try:
                validate(options)
            except SchemaValidationError as error:
                path = ".".join(map(str, error.absolute_path))
                raise ValueError(
                    f"invalid Native profile {name}.{path}: {error.message}"
                ) from error
        return value


class QueryConfig(ConfigModel):
    group_scan_limit: int = Field(
        default_factory=lambda: cast(int, _profile_defaults("query")["group_scan_limit"]),
        description="Maximum candidate rows inspected by each grouped vector lane.",
        json_schema_extra=lambda node: _native_field_schema("query", "group_scan_limit", node),
    )

    @field_validator("group_scan_limit")
    @classmethod
    def native_group_scan_limit_value(cls, value: int) -> int:
        _validate_native_field("query", "group_scan_limit", value)
        return value

    default_mode: Literal["lexical", "dense", "tagmemo", "rivermemo", "hybrid", "dtsc"] = setting(
        "hybrid", description="Default retrieval mode."
    )
    top_k: int = Field(
        default_factory=lambda: cast(int, _profile_defaults("query")["top_k"]),
        description="Default maximum result count.",
        json_schema_extra=lambda node: _native_field_schema("query", "top_k", node),
    )
    include_counterevidence: bool = setting(
        True, description="Include explicitly related counterevidence."
    )

    @field_validator("top_k")
    @classmethod
    def native_top_k_value(cls, value: int) -> int:
        _validate_native_field("query", "top_k", value)
        return value


class ContextConfig(ConfigModel):
    enabled: bool = setting(True, description="Use supplied context for retrieval.")
    window: int = Field(
        default_factory=lambda: cast(int, _profile_defaults("context")["window"]),
        description="Recent fragments retained per role.",
        json_schema_extra=lambda node: _native_field_schema("context", "window", node),
    )
    decay: float = Field(
        default_factory=lambda: cast(float, _profile_defaults("context")["decay"]),
        description="Per-fragment recency decay.",
        json_schema_extra=lambda node: _native_field_schema("context", "decay", node),
    )
    mix: float = Field(
        default_factory=lambda: cast(float, _profile_defaults("context")["context_mix"]),
        description="Context contribution to the query.",
        json_schema_extra=lambda node: _native_field_schema("context", "context_mix", node),
    )
    role_weights: dict[str, float] = Field(
        default_factory=lambda: cast(
            dict[str, float], _profile_defaults("context")["role_weights"]
        ),
        description="Relative contribution of each context role.",
        json_schema_extra=lambda node: _native_field_schema("context", "role_weights", node),
    )
    default_role_weight: float = Field(
        default_factory=lambda: cast(float, _profile_defaults("context")["default_role_weight"]),
        description="Weight for unspecified roles.",
        json_schema_extra=lambda node: _native_field_schema("context", "default_role_weight", node),
    )
    segment_threshold: float = Field(
        default_factory=lambda: cast(float, _profile_defaults("context")["segment_threshold"]),
        description="Adjacent context segment similarity threshold.",
        json_schema_extra=lambda node: _native_field_schema("context", "segment_threshold", node),
    )

    @field_validator(
        "window", "decay", "mix", "role_weights", "default_role_weight", "segment_threshold"
    )
    @classmethod
    def native_context_value(cls, value: object, info: ValidationInfo) -> object:
        name = "context_mix" if info.field_name == "mix" else str(info.field_name)
        _validate_native_field("context", name, value)
        return value


class DedupConfig(ConfigModel):
    semantic_enabled: bool = setting(False, description="Fold semantically similar results.")
    threshold: float = setting(0.92, ge=-1, le=1, description="Semantic folding similarity.")


class FusionConfig(ConfigModel):
    rrf_k: float = Field(
        default_factory=lambda: cast(float, _profile_defaults("query")["rrf_k"]),
        description="Rank fusion smoothing constant.",
        json_schema_extra=lambda node: _native_field_schema("query", "rrf_k", node),
    )

    @field_validator("rrf_k")
    @classmethod
    def native_rrf_value(cls, value: float) -> float:
        _validate_native_field("query", "rrf_k", value)
        return value


class AssociationsConfig(ConfigModel):
    enabled: bool = setting(
        True, apply="new_binding", description="Project positive relationships."
    )
    interpretation_gain: float = setting(
        0.0,
        ge=0,
        apply="new_binding",
        description="Explicit interpretation projection gain.",
    )
    relation_gains: dict[str, float] = setting(
        {"related": 1.0, "supports": 1.0, "sequence": 1.0, "narrative_next": 1.0},
        apply="new_binding",
        description="Retrieval gain for each association type.",
    )

    @field_validator("interpretation_gain")
    @classmethod
    def finite_interpretation_gain(cls, value: float) -> float:
        if not math.isfinite(value):
            raise ValueError("interpretation gain must be finite")
        return value

    @field_validator("relation_gains")
    @classmethod
    def nonnegative_gains(cls, values: dict[str, float]) -> dict[str, float]:
        if any(not math.isfinite(value) or value < 0 for value in values.values()):
            raise ValueError("association gains must be nonnegative")
        return values


class WaveConfig(ConfigModel):
    native: NativeConfig = Field(default_factory=NativeConfig)
    query: QueryConfig = Field(default_factory=QueryConfig)
    context: ContextConfig = Field(default_factory=ContextConfig)
    dedup: DedupConfig = Field(default_factory=DedupConfig)
    fusion: FusionConfig = Field(default_factory=FusionConfig)
    associations: AssociationsConfig = Field(default_factory=AssociationsConfig)
