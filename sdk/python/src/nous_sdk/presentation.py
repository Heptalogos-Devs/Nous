"""Compact query text retains exact references, attribution and uncertainty."""

import json

from .contracts import AssociationBody, CognitiveRecord, EntityBody, MediaRecord, QueryResult
from .contracts.fact_body import FactClaimBody


def reference_text(record: CognitiveRecord | MediaRecord) -> str:
    return record.addresses.exact if record.addresses else record.reference.exact()


def object_text(record: CognitiveRecord | MediaRecord) -> str:
    if isinstance(record, MediaRecord):
        return record.text or f"{record.filename} ({record.media_type})"
    if record.body is None:
        return "purged" if record.purged else ""
    body = record.body
    data = body.model_dump(mode="json")
    lines: list[str] = []
    if isinstance(body, EntityBody):
        lines.extend((body.label, body.profile_text))
        if body.effective_aliases:
            lines.append("aliases: " + ", ".join(body.effective_aliases))
        for attribute in body.attributes:
            lines.append(
                f"{attribute.predicate}: {attribute.value}"
                + " · "
                + attribute.epistemic.model_dump_json(exclude_none=True)
            )
    elif isinstance(body, AssociationBody):
        lines.append(f"{body.from_ref.exact()} — {body.relation} → {body.to_ref.exact()}")
        if body.description:
            lines.append(body.description)
    elif isinstance(body, FactClaimBody):
        claim = f"{body.fact_key}: {body.value_text}"
        lines.append(f"¬({claim})" if body.polarity == "negative" else claim)
        lines.append("scope: " + body.scope.model_dump_json())
    else:
        for key in ("title", "label", "name", "text", "summary", "description", "meaning"):
            if key == "summary" and data.get("text"):
                continue
            value = data.get(key)
            if isinstance(value, str) and value.strip() and value not in lines:
                lines.append(value)
    if not any(lines):
        lines.append(json.dumps(data, ensure_ascii=False, separators=(",", ":")))
    attribution = {
        key: data[key]
        for key in (
            "role",
            "formation_mode",
            "epistemic",
            "status",
            "polarity",
            "basis_class",
            "authority",
            "confidence",
            "cardinality",
            "stability",
        )
        if data.get(key) is not None
    }
    if attribution:
        lines.append(json.dumps(attribution, ensure_ascii=False, separators=(",", ":")))
    if record.times.validity is not None:
        lines.append("validity: " + record.times.validity.model_dump_json())
    return "\n".join(line for line in lines if line)


def query_text(result: QueryResult) -> str:
    parts = [
        f"{reference_text(hit.object)} [{hit.authority}]\n{object_text(hit.object)}"
        for hit in result.hits
    ]
    if result.execution and result.execution.missing_vectors:
        parts.append(f"missing_vectors: {result.execution.missing_vectors}")
    if result.execution and result.execution.reasons:
        parts.append("retrieval: " + ", ".join(result.execution.reasons))
    pending = [result.vcp] if result.vcp else []
    while pending:
        intent = pending.pop()
        pending.extend(reversed(intent.branches))
        if intent.thinking is None:
            continue
        thinking = intent.thinking
        parts.append(
            "thinking: " + thinking.status + (f" ({thinking.reason})" if thinking.reason else "")
        )
        for stage in thinking.stages:
            for selection in stage.selections:
                if selection.route is not None:
                    route = selection.route
                    parts.append(
                        f"{selection.reference.exact()}: "
                        f"{route.source.exact()} → {route.target.exact()}"
                    )
    return "\n\n".join(parts) if parts else "0 results"
