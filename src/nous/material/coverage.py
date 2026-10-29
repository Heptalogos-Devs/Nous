"""A whole-source input dominates region-only requirements for that same source root."""

from collections.abc import Iterable


def merge_source_regions(
    requirements: Iterable[tuple[str, tuple[str, ...]]],
) -> dict[str, tuple[str, ...]]:
    combined: dict[str, set[str] | None] = {}
    for root, regions in requirements:
        if not regions:
            combined[root] = None
        elif root not in combined:
            combined[root] = set(regions)
        elif (existing := combined[root]) is not None:
            existing.update(regions)
    return {root: tuple(sorted(regions)) for root, regions in combined.items() if regions}
