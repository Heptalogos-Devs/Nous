"""Compact presentations of shared typed cognitive results."""

from __future__ import annotations

from typing import TYPE_CHECKING

from pydantic import JsonValue

from .contracts.cognition import CognitiveBody, CognitiveRecord, EntityBody, MemorySemanticsBody
from .contracts.common import CognitiveTimes, Contract
from .contracts.media import MediaReference

if TYPE_CHECKING:
    from .contracts.retrieval import QueryResult


def body_text(body: CognitiveBody | None) -> str:
    if body is None:
        return ""
    if isinstance(body, MemorySemanticsBody):
        return "\n".join([*(item.text for item in body.interpretations), *body.qualifications])
    if isinstance(body, EntityBody):
        return "\n".join((body.label, body.profile_text, *body.effective_aliases))
    value = body.model_dump(mode="json")
    return "\n".join(
        str(value[key])
        for key in (
            "text",
            "summary",
            "label",
            "description",
            "title",
            "narrative",
            "claim",
            "understanding",
            "fact_key",
            "value_text",
            "expression",
            "meaning",
            "content_description",
            "pragmatics",
            "relation",
            "facet",
        )
        if value.get(key)
    )


class QueryItem(Contract):
    ref: str
    kind: str
    title: str
    content: str
    sources: tuple[str, ...]
    time: CognitiveTimes | None = None
    media: MediaReference | None = None


class QueryView(Contract):
    text: str
    items: tuple[QueryItem, ...]
    context_ref: str | None = None
    trace_ref: str | None = None
    next_cursor: str | None = None
    notices: tuple[str, ...] = ()


def present_query(
    result: QueryResult, *, max_chars: int = 12000, item_chars: int = 1600
) -> QueryView:
    remaining = max_chars
    items = []
    paragraphs = []
    for index, hit in enumerate(result.hits):
        record = hit.object
        content = body_text(record.body) if isinstance(record, CognitiveRecord) else record.text
        reference = (
            result.result_refs[index]
            if index < len(result.result_refs)
            else record.reference.exact()
        )
        title = (content.splitlines() or [record.reference.kind])[0][:120]
        excerpt = content[: min(item_chars, max(remaining, 0))]
        remaining -= len(excerpt)
        item = QueryItem(
            ref=reference,
            kind=record.reference.kind,
            title=title,
            content=excerpt,
            sources=hit.source_roots,
            time=record.times,
            media=record.reference if isinstance(record.reference, MediaReference) else None,
        )
        items.append(item)
        paragraphs.append(f"[{reference}] {excerpt}")
        if remaining <= 0:
            break
    notices = (
        tuple(result.execution.reasons) if result.execution and result.execution.degraded else ()
    )
    return QueryView(
        text="\n\n".join(paragraphs) if paragraphs else "No results found.",
        items=tuple(items),
        notices=notices,
    )


def present(value: object) -> dict[str, JsonValue]:
    """The shared compact result used by terminal, MCP and HTTP consumers."""
    from pydantic import BaseModel, TypeAdapter

    from .contracts.context import WorkContext
    from .contracts.corrections import CorrectionResult
    from .contracts.maintenance import MaintenanceRun
    from .contracts.public import ReadView
    from .contracts.retrieval import QueryResult

    result: dict[str, JsonValue]
    if isinstance(value, QueryResult):
        return present_query(value).model_dump(mode="json", exclude_none=True)
    if isinstance(value, CognitiveRecord):
        text = body_text(value.body)
        ref = value.addresses.exact if value.addresses else value.reference.exact()
        return {
            "ref": ref,
            "kind": value.reference.kind,
            "title": (text.splitlines() or [value.reference.kind])[0][:120],
            "text": text,
        }
    if isinstance(value, ReadView):
        ref = value.ref
        if isinstance(value.value, dict):
            addresses = value.value.get("addresses")
            if isinstance(addresses, dict) and isinstance(addresses.get("exact"), str):
                ref = str(addresses["exact"])
        result = {"ref": ref, "kind": value.kind, "title": value.title, "text": value.text}
        if value.next_cursor:
            result["next_cursor"] = value.next_cursor
        if value.sources:
            result["sources"] = [
                {
                    "ref": node.target.reference.exact() if node.target else node.key,
                    "text": node.text,
                    "status": node.status,
                }
                for node in value.sources.nodes
            ]
        return result
    if isinstance(value, WorkContext):
        return {
            "ref": "work:" + value.id,
            "title": value.content.purpose,
            "text": value.content.context_text,
            "state": value.state,
            "questions": list(value.content.unresolved_questions),
            "anchors": [target.reference.exact() for target in value.content.anchors],
        }
    if isinstance(value, MaintenanceRun):
        result = {
            "ref": "maintenance:" + value.id,
            "job": "job:" + value.job_id,
            "status": value.status,
            "text": value.proposal.summary
            if value.proposal
            else value.draft.summary
            if value.draft
            else "",
            "questions": list(value.questions),
        }
        if value.review:
            result["items"] = [
                {"ref": ref.exact(), "kind": ref.kind} for ref in value.review.adopted
            ]
        if value.waiting_reason:
            result["reason"] = value.waiting_reason
        return result
    if isinstance(value, CorrectionResult):
        return {"text": "Correction saved.", "items": [present(record) for record in value.records]}
    raw = (
        value.model_dump(mode="json")
        if isinstance(value, BaseModel)
        else TypeAdapter(JsonValue).validate_python(value)
    )
    if isinstance(raw, dict):
        return raw
    if isinstance(raw, list):
        return {"items": raw}
    return {"text": str(raw) if raw is not None else ""}
