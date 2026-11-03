"""Explicit temporal values and operations without an implicit domain clock."""

from __future__ import annotations

import re
import time
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import UTC, date, datetime, timedelta
from types import MappingProxyType
from typing import Annotated, Literal
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from pydantic import BaseModel, ConfigDict, Field, model_validator

from nous.common.errors import NousError
from nous.common.values import ResourceId


class _Value(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")


class TimePoint(_Value):
    domain_id: ResourceId
    ticks: int = Field(strict=True)


class Known(_Value):
    kind: Literal["known"] = "known"
    at: TimePoint


class Unbounded(_Value):
    kind: Literal["unbounded"] = "unbounded"


class UnknownBoundary(_Value):
    kind: Literal["unknown"] = "unknown"


Boundary = Annotated[Known | Unbounded | UnknownBoundary, Field(discriminator="kind")]


class Unknown(_Value):
    kind: Literal["unknown"] = "unknown"
    source_text: str | None = None


class Instant(_Value):
    kind: Literal["instant"] = "instant"
    at: TimePoint
    source_text: str | None = None


class Interval(_Value):
    kind: Literal["interval"] = "interval"
    start: Boundary
    end: Boundary
    meaning: Literal["duration", "uncertain_instant"]
    precision: str | None = None
    source_text: str | None = None

    @model_validator(mode="after")
    def ordered(self) -> Interval:
        if isinstance(self.start, Known) and isinstance(self.end, Known):
            if compare_points(self.start.at, self.end.at) != "less":
                raise ValueError("interval end must be later than start")
        return self


TemporalValue = Annotated[Unknown | Instant | Interval, Field(discriminator="kind")]


class CognitiveTimes(_Value):
    occurred: TemporalValue | None = None
    observed: TemporalValue | None = None
    formed: TemporalValue | None = None
    valid: TemporalValue | None = None
    recorded: TimePoint | None = None

    @model_validator(mode="after")
    def recorded_is_utc(self) -> CognitiveTimes:
        if self.recorded is not None and self.recorded.domain_id != "utc":
            raise ValueError("recorded time must use UTC")
        return self


class Clock:
    def utc_now(self) -> datetime:
        return datetime.now(UTC)

    def monotonic_ns(self) -> int:
        return time.monotonic_ns()


@dataclass(frozen=True, slots=True, init=False)
class DomainClockSnapshot:
    values: Mapping[str, TimePoint]

    def __init__(self, values: Mapping[str, TimePoint]) -> None:
        if any(key != point.domain_id for key, point in values.items()):
            raise ValueError("clock keys must match their time domains")
        object.__setattr__(self, "values", MappingProxyType(dict(values)))

    def now(self, domain_id: str) -> TimePoint:
        try:
            return self.values[domain_id]
        except KeyError as exc:
            raise NousError(
                "time.clock_required",
                "A current point in this time domain is required",
                details={"domain_id": domain_id},
            ) from exc


def compare_points(a: TimePoint, b: TimePoint) -> Literal["less", "equal", "greater"]:
    if a.domain_id != b.domain_id:
        raise NousError(
            "time.domain_mismatch",
            "Points from different time domains cannot be compared",
            details={"left": a.domain_id, "right": b.domain_id},
        )
    return "less" if a.ticks < b.ticks else "greater" if a.ticks > b.ticks else "equal"


def _zone(name: str) -> ZoneInfo:
    try:
        return ZoneInfo(name)
    except (ZoneInfoNotFoundError, ValueError) as exc:
        raise NousError(
            "time.invalid_timezone", "Unknown time zone", details={"timezone": name}
        ) from exc


def _localize(value: datetime, name: str, fold: int | None = None) -> datetime:
    zone = _zone(name)
    candidates: list[datetime] = []
    for index in (0, 1):
        candidate = value.replace(tzinfo=zone, fold=index)
        if candidate.astimezone(UTC).astimezone(zone).replace(tzinfo=None) == value:
            candidates.append(candidate)
    if not candidates:
        raise NousError(
            "time.nonexistent_local_time", "Local time does not exist in this time zone"
        )
    if len(candidates) == 2 and candidates[0].utcoffset() != candidates[1].utcoffset():
        if fold not in (0, 1):
            raise NousError(
                "time.ambiguous_local_time", "Local time is ambiguous; specify an offset or fold"
            )
        return candidates[fold]
    return candidates[0]


def _utc_point(value: datetime) -> TimePoint:
    if value.tzinfo is None or value.utcoffset() is None:
        raise NousError("time.timezone_required", "A time zone or UTC offset is required")
    delta = value.astimezone(UTC) - datetime(1970, 1, 1, tzinfo=UTC)
    micros = (delta.days * 86_400 + delta.seconds) * 1_000_000 + delta.microseconds
    return TimePoint(domain_id="utc", ticks=micros)


def parse_explicit_time(
    value: str | int | None,
    domain: str = "utc",
    timezone: str | None = None,
    *,
    fold: int | None = None,
) -> Unknown | Instant | Interval:
    if fold not in (None, 0, 1):
        raise NousError("time.invalid_value", "DST fold must be 0 or 1")
    if value is None:
        return Unknown()
    if isinstance(value, bool):
        raise NousError("time.invalid_value", "A boolean is not a time coordinate")
    if isinstance(value, int) or (domain != "utc" and re.fullmatch(r"[+-]?\d+", value)):
        return Instant(at=TimePoint(domain_id=domain, ticks=int(value)), source_text=str(value))
    if domain != "utc":
        raise NousError("time.invalid_value", "A non-UTC domain requires an integer coordinate")
    text = value.strip()
    if not text:
        return Unknown(source_text=value)
    if re.fullmatch(r"\d{4}(?:-\d{2}(?:-\d{2})?)?", text):
        try:
            if len(text) == 4:
                start_date = date(int(text), 1, 1)
                end_date = date(start_date.year + 1, 1, 1)
                precision = "year"
            elif len(text) == 7:
                start_date = date.fromisoformat(text + "-01")
                end_date = (
                    date(start_date.year + 1, 1, 1)
                    if start_date.month == 12
                    else date(start_date.year, start_date.month + 1, 1)
                )
                precision = "month"
            else:
                start_date = date.fromisoformat(text)
                end_date = start_date + timedelta(days=1)
                precision = "day"
        except (ValueError, OverflowError) as exc:
            raise NousError("time.invalid_value", "Invalid calendar date") from exc
        if timezone is None:
            raise NousError("time.timezone_required", "A time zone is required for a calendar date")
        start = _localize(datetime.combine(start_date, datetime.min.time()), timezone)
        end = _localize(datetime.combine(end_date, datetime.min.time()), timezone)
        return Interval(
            start=Known(at=_utc_point(start)),
            end=Known(at=_utc_point(end)),
            meaning="uncertain_instant",
            precision=precision,
            source_text=value,
        )
    try:
        point = datetime.fromisoformat(text)
    except ValueError:
        if re.match(r"^\d{4}-\d", text):
            raise NousError("time.invalid_value", "Invalid ISO time") from None
        return Unknown(source_text=value)
    if point.tzinfo is None or point.utcoffset() is None:
        if timezone is None:
            raise NousError("time.timezone_required", "A time zone or UTC offset is required")
        point = _localize(point, timezone, fold)
    return Instant(at=_utc_point(point), source_text=value)


def _bounds(value: Interval) -> tuple[TimePoint | None, TimePoint | None]:
    return (
        value.start.at if isinstance(value.start, Known) else None,
        value.end.at if isinstance(value.end, Known) else None,
    )


def _require_same_domain(*points: TimePoint | None) -> None:
    first = next((point for point in points if point is not None), None)
    if first is not None:
        for point in points:
            if point is not None:
                compare_points(first, point)


def match_range(
    value: Unknown | Instant | Interval, query_range: Interval
) -> Literal["matches", "no_match", "unknown"]:
    if isinstance(value, Unknown):
        return "unknown"
    qstart, qend = _bounds(query_range)
    if isinstance(value, Instant):
        _require_same_domain(value.at, qstart, qend)
        if (qstart is not None and value.at.ticks < qstart.ticks) or (
            qend is not None and value.at.ticks >= qend.ticks
        ):
            return "no_match"
        if isinstance(query_range.start, UnknownBoundary) or isinstance(
            query_range.end, UnknownBoundary
        ):
            return "unknown"
        return "matches"
    start, end = _bounds(value)
    _require_same_domain(start, end, qstart, qend)
    if (end is not None and qstart is not None and end.ticks <= qstart.ticks) or (
        qend is not None and start is not None and qend.ticks <= start.ticks
    ):
        return "no_match"
    if value.meaning == "uncertain_instant":
        start_covered = isinstance(query_range.start, Unbounded) or (
            start is not None and qstart is not None and qstart.ticks <= start.ticks
        )
        end_covered = isinstance(query_range.end, Unbounded) or (
            end is not None and qend is not None and end.ticks <= qend.ticks
        )
        return "matches" if start_covered and end_covered else "unknown"
    uncertain = any(
        isinstance(boundary, UnknownBoundary)
        for boundary in (value.start, value.end, query_range.start, query_range.end)
    )
    if uncertain:
        if (
            isinstance(query_range.start, Unbounded) and isinstance(query_range.end, Unbounded)
        ) or (isinstance(value.start, Unbounded) and isinstance(value.end, Unbounded)):
            return "matches"
        if (isinstance(value.start, Unbounded) and isinstance(query_range.start, Unbounded)) or (
            isinstance(value.end, Unbounded) and isinstance(query_range.end, Unbounded)
        ):
            return "matches"
        if start is not None and match_range(Instant(at=start), query_range) == "matches":
            return "matches"
        if qstart is not None and match_range(Instant(at=qstart), value) == "matches":
            return "matches"
        return "unknown"
    return "matches"


def _format_point(point: TimePoint, timezone: str) -> str:
    if point.domain_id != "utc":
        return f"{point.domain_id}:{point.ticks}"
    value = datetime(1970, 1, 1, tzinfo=UTC) + timedelta(microseconds=point.ticks)
    rendered = value.astimezone(_zone(timezone)).isoformat()
    return rendered.replace("+00:00", "Z")


def format_time(
    value: Unknown | Instant | Interval, language: str = "en", timezone: str = "UTC"
) -> str:
    if isinstance(value, Unknown):
        return value.source_text or ("未知" if language == "zh-CN" else "unknown")
    if isinstance(value, Instant):
        return _format_point(value.at, timezone)

    def boundary_text(boundary: Boundary, *, lower: bool) -> str:
        if isinstance(boundary, Known):
            return _format_point(boundary.at, timezone)
        if isinstance(boundary, Unbounded):
            return "-∞" if lower else "+∞"
        return "?"

    return f"[{boundary_text(value.start, lower=True)}, {boundary_text(value.end, lower=False)})"
