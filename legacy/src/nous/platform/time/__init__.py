"""Preserve temporal precision instead of guessing a timezone."""

import math
from datetime import UTC, date, datetime
from typing import Literal

from nous_sdk.contracts import TimePoint
from pydantic import BaseModel, JsonValue


class TemporalValue(BaseModel):
    precision: Literal[
        "calendar_year",
        "calendar_month",
        "calendar_date",
        "local_date_time",
        "zoned_instant",
        "unknown",
    ]
    value: str | None = None
    original: str
    timezone: str | None = None


def parse_temporal(value: str, timezone: str | None = None) -> TemporalValue:
    try:
        if len(value) == 4 and value.isdigit():
            date(int(value), 1, 1)
            return TemporalValue(
                precision="calendar_year", value=value, original=value, timezone=timezone
            )
        if len(value) == 7 and value[4] == "-":
            year, month = value.split("-")
            date(int(year), int(month), 1)
            return TemporalValue(
                precision="calendar_month", value=value, original=value, timezone=timezone
            )
        if len(value) == 10:
            parsed_date = date.fromisoformat(value)
            return TemporalValue(
                precision="calendar_date",
                value=parsed_date.isoformat(),
                original=value,
                timezone=timezone,
            )
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
        precision = "zoned_instant" if parsed.utcoffset() is not None else "local_date_time"
        return TemporalValue(
            precision=precision, value=parsed.isoformat(), original=value, timezone=timezone
        )
    except ValueError:
        return TemporalValue(precision="unknown", original=value, timezone=timezone)


def parse_timepoint(expression: str) -> TimePoint | None:
    value = parse_temporal(expression)
    if value.value is None:
        return None
    if value.precision == "calendar_year":
        return TimePoint(
            domain="calendar:gregorian:year",
            precision="year",
            coordinate=int(value.value),
            original=expression,
        )
    if value.precision == "calendar_month":
        year, month = (int(part) for part in value.value.split("-"))
        return TimePoint(
            domain="calendar:gregorian:month",
            precision="month",
            coordinate=year * 12 + month - 1,
            original=expression,
        )
    if value.precision == "calendar_date":
        return TimePoint(
            domain="calendar:gregorian:date",
            precision="day",
            coordinate=date.fromisoformat(value.value).toordinal(),
            original=expression,
        )
    point = datetime.fromisoformat(value.value)
    if value.precision == "zoned_instant":
        return TimePoint(
            domain="unix_ms",
            coordinate=int(point.astimezone(UTC).timestamp() * 1000),
            original=expression,
        )
    # A wall-clock coordinate has its own domain and never implies an instant in UTC.
    coordinate = (
        point.toordinal() * 86_400_000
        + point.hour * 3_600_000
        + point.minute * 60_000
        + point.second * 1000
        + point.microsecond // 1000
    )
    return TimePoint(domain="calendar:gregorian:local", coordinate=coordinate, original=expression)


def parse_source_time(
    value: JsonValue, *, numeric_unit: Literal["seconds", "milliseconds"] = "seconds"
) -> TimePoint | None:
    """Decode explicitly declared source units while retaining calendar precision."""
    if value is None or value == "":
        return None
    if isinstance(value, dict):
        return TimePoint.model_validate(value)
    if isinstance(value, bool):
        raise ValueError("A source timestamp cannot be a boolean")
    if isinstance(value, (int, float)):
        if not math.isfinite(value):
            raise ValueError("Source timestamps must be finite")
        return TimePoint(
            coordinate=round(value * (1000 if numeric_unit == "seconds" else 1)),
            original=str(value),
        )
    if isinstance(value, str):
        return parse_timepoint(value)
    raise ValueError("Source timestamp must contain a declared coordinate or date")
