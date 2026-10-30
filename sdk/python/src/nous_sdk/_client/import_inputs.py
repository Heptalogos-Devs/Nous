"""Normalize the supported public document envelopes into current import contracts."""

from collections.abc import Mapping
from typing import Annotated, Literal

from pydantic import Field, JsonValue, TypeAdapter

from ..contracts import Contract, ImportDocument, TimePoint
from ..operations import object_value
from ..types import JsonObject


class InlineInput(Contract):
    kind: Literal["inline"]
    content: str


class MaterialInput(Contract):
    kind: Literal["material"]
    material_id: str


class DocumentInput(Contract):
    name: str | None = None
    format: Literal["text", "json", "jsonl"]
    input: Annotated[InlineInput | MaterialInput, Field(discriminator="kind")]
    source: str | None = None


class SplitOverrides(Contract):
    strategy: Literal["auto", "factual", "narrative", "quote", "chat_log"] | None = None
    narrative_window_size: int | None = None
    narrative_overlap: int | None = None
    factual_target_size: int | None = None
    reference_time: TimePoint | None = None


def origin(value: JsonValue) -> dict[str, str]:
    if value is None:
        return {}
    fields = TypeAdapter(dict[str, str | None]).validate_python(value)
    return {key: item for key, item in fields.items() if item is not None}


def document(value: ImportDocument | Mapping[str, JsonValue]) -> ImportDocument:
    if isinstance(value, ImportDocument):
        return value
    values: JsonObject = dict(value)
    identity: JsonValue = None
    if "document" in values:
        unknown = set(values).difference({"document", "file_identity"})
        if unknown:
            raise ValueError("Unknown file document envelope fields: " + ", ".join(sorted(unknown)))
        identity = values.get("file_identity")
        values = object_value(values["document"])
    if "input" in values:
        parsed = DocumentInput.model_validate(values)
        values = {
            "filename": parsed.name or "input.txt",
            "format": parsed.format,
            "source": parsed.source,
            **(
                {"text": parsed.input.content}
                if isinstance(parsed.input, InlineInput)
                else {"material_id": parsed.input.material_id}
            ),
        }
    if "origin" in values:
        values["origin"] = {key: item for key, item in origin(values["origin"]).items()}
    if identity is not None:
        values["file_identity"] = identity
    return ImportDocument.model_validate(values)


def normalize_inputs(values: JsonObject) -> None:
    raw = values.get("inputs")
    if not isinstance(raw, list):
        raise ValueError("Import inputs require an ordered document list")
    selected = [document(object_value(item)) for item in raw]
    default_origin = origin(values.pop("origin", None))
    values["inputs"] = [
        {
            **item.model_dump(mode="json"),
            "origin": {**default_origin, **item.origin},
        }
        for item in selected
    ]


def split_options(values: JsonObject) -> None:
    raw = values.get("inputs")
    legacy = "overrides" in values or (
        isinstance(raw, list)
        and any(
            isinstance(item, dict)
            and (
                "input" in item
                or (isinstance(nested := item.get("document"), dict) and "input" in nested)
            )
            for item in raw
        )
    )
    explicit_fields = set(values)
    if legacy:
        for key, default in {
            "strategy_override": "auto",
            "narrative_window_size": 1600,
            "narrative_overlap": 400,
            "factual_target_size": 1200,
        }.items():
            values.setdefault(key, default)
    overrides = values.pop("overrides", None)
    if overrides is None:
        return
    parsed = SplitOverrides.model_validate(overrides)
    for key, item in parsed.model_dump(mode="json", exclude_none=True).items():
        target = "strategy_override" if key == "strategy" else key
        if target in explicit_fields and values[target] != item:
            raise ValueError("Conflicting explicit import split options")
        values[target] = item


def validate_flags(values: JsonObject) -> None:
    for key in (
        "form_memory",
        "llm_enabled",
        "extract_time",
        "force",
        "clear_manifest",
        "deferred_formation",
    ):
        if key in values:
            TypeAdapter(bool).validate_python(values[key], strict=True)
