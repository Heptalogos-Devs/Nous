"""Natural language views of a fixed catalog; authority metadata stays in the run."""

from datetime import UTC, datetime

from nous_sdk.contracts import EpistemicStance, TimePoint, TimeRange
from nous_sdk.contracts.growth import GrowthSnapshot
from nous_sdk.presentation import body_text


def time_text(point: TimePoint | None) -> str:
    if point is None:
        return ""
    if point.original:
        return point.original
    if point.domain != "unix_ms":
        return f"{point.domain}：{point.coordinate}"
    value = datetime.fromtimestamp(point.coordinate / 1000, UTC)
    return value.strftime(
        {"year": "%Y年", "month": "%Y年%m月", "day": "%Y年%m月%d日"}.get(
            point.precision, "%Y-%m-%d %H:%M UTC"
        )
    )


def range_text(period: TimeRange | None) -> str:
    if period is None:
        return ""
    start = (
        time_text(TimePoint(domain=period.domain, coordinate=period.start))
        if period.start is not None
        else ""
    )
    end = (
        time_text(TimePoint(domain=period.domain, coordinate=period.end))
        if period.end is not None
        else ""
    )
    return f"{start} 至 {end}" if start or end else ""


def materials(snapshot: GrowthSnapshot) -> str:
    paragraphs: list[str] = []
    nature = {
        "observed": "实际记录",
        "reported": "转述或自述",
        "inferred": "已有推断",
        "derived": "已有归纳",
        "simulated": "创作设定",
        "narrative": "叙事材料",
    }
    for entry in snapshot.entries:
        record = entry.record
        stance = getattr(record.body, "epistemic", None)
        labels: list[str] = []
        if isinstance(stance, EpistemicStance):
            if stance.classification:
                labels.append(nature.get(stance.classification, stance.classification))
            attribution = stance.attribution
            if attribution and attribution != record.producer_call_id:
                labels.append("归属：" + attribution)
        if stamp := time_text(record.times.event):
            labels.append("所述时间：" + stamp)
        paragraphs.append(
            "[" + entry.key + "] " + "；".join(labels) + "\n" + body_text(record.body)
        )
    for source in snapshot.sources:
        stamp = time_text(source.times.observation)
        paragraphs.append(
            "["
            + source.key
            + "] 原始材料"
            + ("；资料接触时间：" + stamp if stamp else "")
            + "\n"
            + source.text
        )
    return "\n\n".join(paragraphs)
