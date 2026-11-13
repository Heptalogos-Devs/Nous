"""Select fields from serialized operation results using JSON Pointer."""

from collections.abc import Sequence

from jsonpointer import JsonPointer, JsonPointerException, resolve_pointer
from pydantic import JsonValue, TypeAdapter, ValidationError
from pydantic_core import to_jsonable_python

from nous.common.errors import NousError

_JSON = TypeAdapter(JsonValue)


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
    document = _JSON.validate_python(to_jsonable_python(value, round_trip=False))
    if not pointers:
        return document
    result: dict[str, JsonValue] = {}
    for pointer in validate_json_pointers(pointers):
        try:
            result[pointer] = _JSON.validate_python(resolve_pointer(document, pointer))
        except (JsonPointerException, ValidationError, TypeError, ValueError) as error:
            raise NousError(
                "input.invalid_selection",
                "JSON pointer does not identify a result field",
                details={"pointer": pointer},
            ) from error
    return result
