"""Present a retrieval outcome without changing its exact source ranges."""

from typing import TYPE_CHECKING

from nous.content.contracts import ContentReadView, ResultOutputRef
from nous.presentation import translate
from nous.wave.contracts import DeliveredText

if TYPE_CHECKING:
    from nous.cognition.material_contracts import MaterialRead, MaterialRef
    from nous.cognition.retrieval.contracts import RetrievalOutcome


def _reference(value: MaterialRef) -> str:
    if isinstance(value, ResultOutputRef):
        return f"{value.result.identifier}:{value.output}"
    return value.identifier


def format_source_text(
    fragments: tuple[DeliveredText, ...],
    support: tuple[DeliveredText, ...],
    seen: dict[str, list[tuple[int, int]]] | None = None,
) -> str:
    """Render non-overlapping source bytes in document order, once per output."""
    if seen is None:
        seen = {}
    order: dict[str, int] = {}
    items: dict[str, DeliveredText] = {}
    for part in (*fragments, *support):
        order.setdefault(part.content.artifact.id, len(order))
        items.setdefault(part.content.model_dump_json(), part)
    values = sorted(
        items.values(),
        key=lambda item: (
            order[item.content.artifact.id],
            item.content.selection.start_byte
            if item.content.selection.start_byte is not None
            else -1,
            item.content.selection.end_byte if item.content.selection.end_byte is not None else -1,
        ),
    )
    output: list[str] = []
    last_artifact: str | None = None
    last_end: int | None = None
    for part in values:
        identity = part.content.artifact.id
        start, end = part.content.selection.start_byte, part.content.selection.end_byte
        data = part.text.encode("utf-8")
        if start is None or end is None or end - start != len(data):
            if part.text:
                output.append(("\n" if output else "") + part.text)
            continue
        remaining = [(start, end)]
        for left, right in seen.get(identity, []):
            remaining = [(a, min(b, left)) for a, b in remaining if a < left] + [
                (max(a, right), b) for a, b in remaining if b > right
            ]
        for left, right in sorted(remaining):
            if left >= right:
                continue
            prefix = (
                ""
                if not output
                else "\n"
                if identity != last_artifact
                else ""
                if left == last_end
                else "…"
            )
            output.append(prefix + data[left - start : right - start].decode("utf-8"))
            last_artifact, last_end = identity, right
        seen.setdefault(identity, []).append((start, end))
    return "".join(output)


def format_retrieval(
    outcome: RetrievalOutcome, language: str = "zh-CN", *, include_issues: bool = True
) -> str:
    """Render a reading brief or the actual evidence with its source identities."""
    lines: list[str] = []
    if outcome.reading is not None:
        if outcome.reading.title:
            lines.append(outcome.reading.title)
        for item in outcome.reading.items:
            lines.append(
                item.markdown
                + (f" [{', '.join(item.evidence_keys)}]" if item.evidence_keys else "")
            )
    else:
        passages = {item.key: item for item in outcome.evidence.passages}
        seen: dict[str, list[tuple[int, int]]] = {}
        if not outcome.evidence.materials:
            coverage = outcome.evidence.coverage
            label = (
                "No material selected"
                if coverage is None or coverage.truncated or (coverage.ranked_candidates or 0) > 0
                else "No matching knowledge"
            )
            lines.append(translate(label, language))
        for item in outcome.evidence.materials:
            fallback = (
                "Untitled work"
                if item.material.kind == "work"
                else ("Untitled journal" if item.material.kind == "journal" else "Untitled note")
            )
            lines.append(
                f"{item.rank}. {item.material.title or translate(fallback, language)} — {_reference(item.material.ref)}"
            )
            for key in item.passage_keys:
                passage = passages[key]
                text = format_source_text(passage.fragments, passage.support_context, seen)
                if text:
                    lines.append(f"[{key}] {text}")
            if not item.passage_keys:
                labels = {
                    "budget_exhausted": "Delivery budget exhausted",
                    "overlap": "Passage already included above",
                    "no_candidates": "No passage located",
                    "not_selected": "No passage selected",
                    "selected": "No passage selected",
                }
                lines.append(translate(labels[item.delivery_status], language))
            lines.append("")
    if outcome.evidence_ref:
        lines.append(f"{translate('Evidence', language)}: {outcome.evidence_ref.identifier}")
    if outcome.reading_ref:
        lines.append(f"{translate('Reading', language)}: {outcome.reading_ref.identifier}")
    for link in outcome.evidence.continuation:
        label = translate(
            {"read": "Read material", "read_remaining": "Read remaining"}.get(
                link.relation, link.relation
            ),
            language,
        )
        lines.append(f"{label}: {_reference(link.target)}")
    if include_issues:
        lines.extend(f"{issue.code}: {issue.message}" for issue in outcome.issues)
    return "\n".join(lines).rstrip()


def format_material_read(value: MaterialRead, language: str = "zh-CN") -> str:
    """Render material and any explicitly requested sources in source order."""
    lines = [
        f"{value.material.title or translate('Material', language)} — {_reference(value.material.ref)}"
    ]
    seen: dict[str, list[tuple[int, int]]] = {}

    def append_views(views: tuple[ContentReadView, ...]) -> None:
        texts: list[DeliveredText] = []
        for item in views:
            if item.body_text is None:
                lines.append(f"{item.media_type} — {item.export_ref.identifier}")
            else:
                texts.append(DeliveredText(content=item.source, text=item.body_text))
        text = format_source_text(tuple(texts), (), seen)
        if text:
            lines.append(text)

    append_views(value.selections)
    for source in value.sources:
        lines.append(
            f"{source.source.key}: {source.source.name or translate(source.source.kind, language)} — {_reference(source.source.material)}"
        )
        append_views(source.views)
    return "\n".join(lines)
