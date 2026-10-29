"""Source coordinates address decoded record text, including JSON/JSONL originals."""

import json

from jsonpointer import JsonPointerException, resolve_pointer
from nous_wave.contracts import DomainError, SourceRegion
from pydantic import JsonValue, TypeAdapter


def source_region_text(content: str, region: SourceRegion) -> str:
    value = content
    if region.line_number is not None:
        lines = value.splitlines()
        if not 1 <= region.line_number <= len(lines):
            raise DomainError("region_unavailable", "JSONL source line is outside the original")
        value = lines[region.line_number - 1]
    if region.json_pointer is not None:
        try:
            decoded = TypeAdapter(JsonValue).validate_python(json.loads(value))
            selected = TypeAdapter(JsonValue).validate_python(
                resolve_pointer(decoded, region.json_pointer)
            )
        except (json.JSONDecodeError, JsonPointerException) as error:
            raise DomainError(
                "region_unavailable", "Source pointer does not resolve in the original"
            ) from error
        if not isinstance(selected, str):
            raise DomainError(
                "region_unavailable", "Text region must address a string in its original"
            )
        value = selected
    if region.text_start is not None or region.text_end is not None:
        start = region.text_start if region.text_start is not None else 0
        end = region.text_end if region.text_end is not None else len(value)
        if not 0 <= start < end <= len(value):
            raise DomainError(
                "region_unavailable", "Text coordinates are outside the decoded source string"
            )
        value = value[start:end]
    return value
