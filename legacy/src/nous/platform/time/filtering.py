"""Shared five-axis time predicates over authorized records."""

from typing import Protocol

from nous_sdk.contracts import CognitiveTimes, TimeFilter


class TimedRecord(Protocol):
    @property
    def times(self) -> CognitiveTimes: ...


def time_matches(record: TimedRecord, filters: tuple[TimeFilter, ...]) -> bool:
    for condition in filters:
        value = getattr(record.times, condition.axis)
        if value is None:
            return False
        if condition.axis == "validity":
            if condition.range.domain != value.domain:
                return False
            if (
                value.end is not None
                and condition.range.start is not None
                and value.end <= condition.range.start
            ):
                return False
            if (
                condition.range.end is not None
                and value.start is not None
                and condition.range.end <= value.start
            ):
                return False
        elif not condition.range.contains(value):
            return False
    return True
