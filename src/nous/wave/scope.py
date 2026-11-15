"""Bind candidate and time scope to exact rows of one Wave snapshot."""

from nous.common.errors import NousError
from nous.temporal.values import CognitiveTimes, Instant, Unknown, match_range

from .contracts import CandidateFilter
from .mapping import SnapshotMapping


def allowed_rows(mapping: SnapshotMapping, requested: CandidateFilter | None) -> list[int]:
    candidates = mapping["candidates"]
    if requested is None:
        return list(range(len(candidates)))
    available = {item["key"] for item in candidates}
    selected = set(requested.candidate_keys) if requested.candidate_keys is not None else available
    unknown = selected - available
    if unknown:
        raise NousError(
            "wave.candidate_not_found",
            "Candidate filter refers to unknown keys",
            details={"candidates": sorted(unknown)},
        )
    rows: list[int] = []
    for index, candidate in enumerate(candidates):
        if candidate["key"] not in selected:
            continue
        if requested.times:
            # All time conditions must match one actual source/declaration, not a mixture.
            root_keys = set(candidate["roots"])
            groups = [
                document
                for key, document in mapping["documents"].items()
                if key == candidate["content_document"] or root_keys.intersection(document["roots"])
            ]
            matches_group = False
            for group in groups:
                times = CognitiveTimes.model_validate(group["times"]) if group["times"] else None
                states: list[str] = []
                for condition in requested.times:
                    value = getattr(times, condition.axis) if times is not None else None
                    if condition.axis == "recorded" and value is not None:
                        value = Instant(at=value)
                    states.append(match_range(value or Unknown(), condition.range))
                if all(
                    state == "matches" or (state == "unknown" and requested.include_unknown)
                    for state in states
                ):
                    matches_group = True
                    break
            if not matches_group:
                continue
        rows.append(index)
    return rows
