"""Select fields from serialized operation results using JSON Pointer."""

from collections.abc import Sequence
from typing import cast

from jsonpointer import JsonPointer, JsonPointerException, resolve_pointer
from pydantic import JsonValue
from pydantic_core import to_jsonable_python

from nous.common.errors import NousError


def validate_json_pointers(pointers: Sequence[str]) -> tuple[str, ...]:
    """Validate pointer syntax before an operation can produce side effects."""
    unique = tuple(dict.fromkeys(pointers))
    for pointer in unique:
        try:
            JsonPointer(pointer)
        except (JsonPointerException, TypeError, ValueError) as error:
            raise NousError(
                "input.invalid_selection",
                "Invalid JSON pointer",
                details={"pointer": pointer},
            ) from error
    return unique


def select_json(value: object, pointers: Sequence[str] = ()) -> JsonValue:
    """Return the complete JSON value or the values at the requested pointers."""
    document = cast(
        JsonValue, to_jsonable_python(value, round_trip=False, exclude_none=not pointers)
    )
    if not pointers:
        return document
    result: dict[str, JsonValue] = {}
    for pointer in dict.fromkeys(pointers):
        try:
            result[pointer] = cast(JsonValue, resolve_pointer(document, pointer))
        except (JsonPointerException, TypeError, ValueError) as error:
            raise NousError(
                "input.invalid_selection",
                "JSON pointer does not identify a result field",
                details={"pointer": pointer},
            ) from error
    return result
