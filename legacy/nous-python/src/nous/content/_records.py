"""Date conversion for persisted content records."""

from datetime import datetime


def date_value(value: object) -> datetime | None:
    return datetime.fromisoformat(str(value)) if value else None
