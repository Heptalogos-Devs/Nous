"""Exact differences and coverage of formally selected source ranges."""

from collections.abc import Sequence

from nous.common.errors import NousError

from .contracts import ArtifactRef, ContentSelection, SourceSelection


def ordered_selections(values: Sequence[ContentSelection]) -> tuple[ContentSelection, ...]:
    """Keep each exact selection once in artifact and original coordinate order."""
    unique = tuple(dict.fromkeys(values))
    artifact_order = {
        ref: index for index, ref in enumerate(dict.fromkeys(item.artifact for item in unique))
    }

    def position(item: ContentSelection) -> tuple[int, int]:
        bounds = item.selection
        start = next(
            (
                value
                for value in (
                    bounds.start_byte,
                    bounds.start_line,
                    bounds.pages[0] if bounds.pages else None,
                    bounds.start_ms,
                    bounds.frame,
                )
                if value is not None
            ),
            0,
        )
        return artifact_order[item.artifact], start

    return tuple(sorted(unique, key=position))


def _subtract_range(
    start: int, end: int | None, covered_start: int, covered_end: int | None
) -> tuple[tuple[int, int | None], ...]:
    if end is not None and start == end:
        return ()
    if (end is not None and end <= covered_start) or (
        covered_end is not None and covered_end <= start
    ):
        return ((start, end),)
    remaining: list[tuple[int, int | None]] = []
    if covered_start > start:
        remaining.append((start, covered_start if end is None else min(end, covered_start)))
    if covered_end is not None and (end is None or covered_end < end):
        remaining.append((max(start, covered_end), end))
    return tuple(remaining)


def _subtract_one(
    origin: ContentSelection, covered: ContentSelection
) -> tuple[ContentSelection, ...]:
    if origin.artifact != covered.artifact:
        return (origin,)
    value, other = origin.selection, covered.selection
    try:
        if other.restrict(value) == value:
            return ()
    except NousError:
        pass
    selectors: tuple[SourceSelection, ...] | None = None
    if value.pages and other.pages:
        pages = tuple(page for page in value.pages if page not in other.pages)
        selectors = (SourceSelection(pages=pages),) if pages else ()
    elif (value.start_byte is not None or value.end_byte is not None) and (
        other.start_byte is not None or other.end_byte is not None
    ):
        selectors = tuple(
            SourceSelection(start_byte=start, end_byte=end)
            for start, end in _subtract_range(
                value.start_byte or 0, value.end_byte, other.start_byte or 0, other.end_byte
            )
        )
    elif (value.start_line is not None or value.end_line is not None) and (
        other.start_line is not None or other.end_line is not None
    ):
        selectors = tuple(
            SourceSelection(start_line=start, end_line=end - 1 if end else None)
            for start, end in _subtract_range(
                value.start_line or 1,
                value.end_line + 1 if value.end_line else None,
                other.start_line or 1,
                other.end_line + 1 if other.end_line else None,
            )
        )
    elif (
        (value.start_ms is not None or value.end_ms is not None)
        and (other.start_ms is not None or other.end_ms is not None)
        and value.frame is None
        and other.frame is None
        and value.crop is None
        and other.crop is None
    ):
        selectors = tuple(
            SourceSelection(start_ms=start, end_ms=end)
            for start, end in _subtract_range(
                value.start_ms or 0, value.end_ms, other.start_ms or 0, other.end_ms
            )
        )
    if selectors is None:
        return (origin,)
    return tuple(
        ContentSelection(artifact=origin.artifact, selection=selector) for selector in selectors
    )


def subtract_selection(
    origin: ContentSelection, covers: Sequence[ContentSelection]
) -> tuple[ContentSelection, ...]:
    """Return the portions not covered by the supplied selections, in source order.

    Text bytes, lines, pages and pure time ranges can be split exactly. When a
    partial overlap in another coordinate family has no representable difference,
    the original portion is retained so callers never silently lose evidence.
    """
    remaining = (origin,)
    for cover in covers:
        remaining = tuple(part for item in remaining for part in _subtract_one(item, cover))
    return remaining


def union_selections(values: Sequence[ContentSelection]) -> tuple[ContentSelection, ...]:
    """Return the source-ordered union, retaining gaps and exact artifact identities."""
    groups: dict[ArtifactRef, list[ContentSelection]] = {}
    for value in values:
        groups.setdefault(value.artifact, []).append(value)
    output: list[ContentSelection] = []
    for items in groups.values():
        whole = next((item for item in items if item.selection.is_whole), None)
        if whole is not None:
            output.append(whole)
            continue
        selected: list[ContentSelection] = []
        for item in ordered_selections(items):
            for part in subtract_selection(item, selected):
                bounds = part.selection
                if bounds.start_byte is not None and bounds.end_byte == bounds.start_byte:
                    continue
                if selected:
                    previous = selected[-1]
                    left, right = previous.selection, part.selection
                    if (
                        left.start_byte is not None
                        and left.end_byte is not None
                        and right.start_byte == left.end_byte
                        and right.end_byte is not None
                    ):
                        selected[-1] = ContentSelection(
                            artifact=part.artifact,
                            selection=SourceSelection(
                                start_byte=left.start_byte, end_byte=right.end_byte
                            ),
                        )
                        continue
                selected.append(part)
        output.extend(selected)
    return tuple(output)
