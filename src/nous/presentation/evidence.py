"""Present a retrieval outcome without changing its exact source ranges."""

import json
from typing import TYPE_CHECKING

from nous.cognition.material_contracts import material_kind
from nous.common.operations import OperationIssue
from nous.content.contracts import ContentReadView, ResultOutputRef
from nous.presentation import translate

if TYPE_CHECKING:
    from nous.cognition.material_contracts import MaterialRead
    from nous.cognition.retrieval.contracts import RetrievalOutcome
    from nous.content.contracts import MaterialRef


def _reference(value: MaterialRef) -> str:
    if isinstance(value, ResultOutputRef):
        return f"{value.result.identifier}:{value.output}"
    return value.identifier


def format_source_text(
    fragments: tuple[ContentReadView, ...],
    support: tuple[ContentReadView, ...],
    seen: dict[str, list[tuple[int, int]]] | None = None,
) -> str:
    """Render non-overlapping source bytes in document order, once per output."""
    if seen is None:
        seen = {}
    order: dict[str, int] = {}
    items: dict[str, ContentReadView] = {}
    for part in (*fragments, *support):
        order.setdefault(part.source.artifact.id, len(order))
        items.setdefault(part.source.model_dump_json(), part)
    values = sorted(
        items.values(),
        key=lambda item: (
            order[item.source.artifact.id],
            item.source.selection.start_byte
            if item.source.selection.start_byte is not None
            else -1,
            item.source.selection.end_byte if item.source.selection.end_byte is not None else -1,
        ),
    )
    output: list[str] = []
    last_artifact: str | None = None
    last_end: int | None = None
    for part in values:
        identity = part.source.artifact.id
        start, end = part.source.selection.start_byte, part.source.selection.end_byte
        data = (part.body_text or "").encode("utf-8")
        if start is None or end is None or end - start != len(data):
            if part.body_text:
                output.append(("\n" if output else "") + part.body_text)
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
        seen: dict[str, list[tuple[int, int]]] = {}
        if not outcome.evidence.materials:
            coverage = outcome.evidence.coverage
            label = (
                "No material selected"
                if coverage is None or coverage["truncated"] or coverage.get("matched") is not False
                else "No matching knowledge"
            )
            lines.append(translate(label, language))
        for item in outcome.evidence.materials:
            fallback = (
                "Untitled work"
                if material_kind(item.material) == "work"
                else (
                    "Untitled journal"
                    if material_kind(item.material) == "journal"
                    else "Untitled note"
                )
            )
            prefix = f"{item.rank}. " if item.rank is not None else ""
            lines.append(
                f"{prefix}{item.title or translate(fallback, language)} — {_reference(item.material)}"
            )
            if item.needs_review:
                lines.append(translate("Needs review", language))
            for passage in item.passages:
                text = format_source_text(passage.views, (), seen)
                if text:
                    lines.append(f"[{passage.key}] {text}")
                elif passage.shared and not passage.views:
                    lines.append(
                        f"[{passage.key}] {translate('Passage already included above', language)}"
                    )
                if passage.shared and passage.views:
                    lines.append(
                        f"[{passage.key}] {translate('Shared context is shown above', language)}"
                    )
                for view in passage.views:
                    if view.body_text is None:
                        lines.append(
                            f"[{passage.key}] {view.media_type} — {view.source.artifact.identifier}"
                        )
            if not item.passages and item.delivery_status is not None:
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
    if outcome.evidence.fallback_from is not None:
        lines.append(
            translate(
                "Program {requested} unavailable; using {program}; missing capabilities: {capabilities}",
                language,
            ).format(
                program=outcome.evidence.program,
                requested=outcome.evidence.fallback_from,
                capabilities=", ".join(outcome.evidence.unavailable_capabilities),
            )
        )
    for link in outcome.evidence.continuation:
        label = translate(
            {"read": "Read material", "read_remaining": "Read remaining"}.get(
                link.relation, link.relation
            ),
            language,
        )
        lines.append(f"{label}: {_reference(link.selection.material)}")
        for part in link.selection.selections:
            lines.append(
                f"  {part.artifact.identifier} {part.selection.model_dump_json(exclude_none=True, exclude_defaults=True)}"
            )
    if include_issues:
        lines.extend(
            format_issue(issue, language) for issue in (*outcome.evidence.issues, *outcome.issues)
        )
    return "\n".join(lines).rstrip()


def format_issue(issue: OperationIssue, language: str = "zh-CN") -> str:
    """Keep the actionable service details with an incomplete stage."""
    details = (
        f" — {json.dumps(issue.details, ensure_ascii=False)}"
        if issue.details and issue.code != "cognition.snapshot_changed"
        else ""
    )
    return f"{issue.code}: {translate(issue.message, language)}{details}"


def format_material_read(value: MaterialRead, language: str = "zh-CN") -> str:
    """Render material and any explicitly requested sources in source order."""
    lines = [
        f"{value.material.title or translate('Material', language)} — {_reference(value.material.ref)}"
    ]
    seen: dict[str, list[tuple[int, int]]] = {}

    def append_views(views: tuple[ContentReadView, ...]) -> None:
        texts: list[ContentReadView] = []
        for item in views:
            if item.body_text is None:
                lines.append(f"{item.media_type} — {item.source.artifact.identifier}")
            else:
                texts.append(item)
        text = format_source_text(tuple(texts), (), seen)
        if text:
            lines.append(text)

    append_views(value.selections)
    for section in value.material.sections or ():
        heading = " / ".join(section.heading_path)
        lines.append(f"{section.key}: {heading} — {section.selection.selection.model_dump_json()}")
    for source in value.material.material_sources:
        if source.key not in value.sources:
            continue
        lines.append(
            f"{source.key}: {source.name or translate(source.kind, language)} — {_reference(source.material)}"
        )
        append_views(value.sources[source.key])
    return "\n".join(lines)
