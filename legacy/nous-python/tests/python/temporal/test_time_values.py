"""The local runtime's user-visible path and its persistent contracts."""

import pytest

from nous.common.errors import NousError
from nous.temporal.values import (
    Instant,
    Interval,
    Known,
    TimePoint,
    Unbounded,
    UnknownBoundary,
    compare_points,
    match_range,
    parse_explicit_time,
)


def test_temporal_precision_unknown_boundaries_and_dst() -> None:
    day = parse_explicit_time("2026-11-03", timezone="Asia/Shanghai")
    assert isinstance(day, Interval) and isinstance(day.start, Known)
    beginning = day.start.at.ticks
    hour = Interval(
        start=Known(at=TimePoint(domain_id="utc", ticks=beginning)),
        end=Known(at=TimePoint(domain_id="utc", ticks=beginning + 3_600_000_000)),
        meaning="duration",
    )
    assert match_range(day, hour) == "unknown"
    assert match_range(day, day) == "matches"
    open_query = Interval(start=Unbounded(), end=Unbounded(), meaning="duration")
    uncertain = Interval(start=UnknownBoundary(), end=UnknownBoundary(), meaning="duration")
    assert match_range(uncertain, open_query) == "matches"
    assert match_range(Instant(at=TimePoint(domain_id="utc", ticks=beginning)), hour) == "matches"
    with pytest.raises(NousError) as mismatch:
        compare_points(TimePoint(domain_id="mars", ticks=0), TimePoint(domain_id="utc", ticks=0))
    assert mismatch.value.code == "time.domain_mismatch"
    with pytest.raises(NousError) as nonexistent:
        parse_explicit_time("2026-03-08T02:30:00", timezone="America/New_York")
    assert nonexistent.value.code == "time.nonexistent_local_time"
    with pytest.raises(NousError) as ambiguous:
        parse_explicit_time("2026-11-01T01:30:00", timezone="America/New_York")
    assert ambiguous.value.code == "time.ambiguous_local_time"
    early = parse_explicit_time("2026-11-01T01:30:00", timezone="America/New_York", fold=0)
    late = parse_explicit_time("2026-11-01T01:30:00", timezone="America/New_York", fold=1)
    assert isinstance(early, Instant) and isinstance(late, Instant)
    assert late.at.ticks - early.at.ticks == 3_600_000_000
