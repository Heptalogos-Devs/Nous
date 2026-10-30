import math
from datetime import datetime

from nous_sdk.contracts import DomainError, TimePoint
from pydantic import JsonValue, TypeAdapter

from nous.platform.persistence.authority import canonical_json


def json_value(value: object) -> JsonValue:
    return TypeAdapter(JsonValue).validate_json(canonical_json(value))


def json_object(value: object) -> dict[str, JsonValue]:
    return TypeAdapter(dict[str, JsonValue]).validate_python(json_value(value))


def point(value: str | float | None) -> TimePoint | None:
    if value is None:
        return None
    if isinstance(value, (int, float)):
        if not math.isfinite(value):
            raise DomainError("time_invalid", "Unix seconds must be finite")
        return TimePoint(coordinate=round(value * 1000))
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise DomainError(
            "time_invalid", "Time requires Unix seconds or an ISO timestamp"
        ) from error
    if parsed.tzinfo is None:
        raise DomainError("time_invalid", "Timestamp requires its explicit timezone")
    return TimePoint(coordinate=round(parsed.timestamp() * 1000))
