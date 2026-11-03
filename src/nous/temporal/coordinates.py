"""Pure validation shared by configured and persisted time domains."""

from fractions import Fraction
from typing import Literal


def validate_coordinates(
    domain_id: str,
    kind: Literal["utc", "linear", "opaque"],
    tick_seconds: str | Fraction | None,
    epoch: str | None,
) -> None:
    try:
        tick = Fraction(tick_seconds) if tick_seconds is not None else None
    except (ValueError, ZeroDivisionError) as error:
        raise ValueError("tick_seconds must be a positive rational") from error
    if kind in {"linear", "utc"} and (tick is None or tick <= 0):
        raise ValueError("linear and utc domains require positive tick_seconds")
    if kind == "opaque" and tick is not None:
        raise ValueError("opaque domains do not define tick_seconds")
    if (kind == "utc" or domain_id == "utc") and (
        domain_id != "utc"
        or kind != "utc"
        or tick != Fraction(1, 1_000_000)
        or epoch != "Unix epoch"
    ):
        raise ValueError("utc uses the fixed Unix epoch and microsecond coordinates")
