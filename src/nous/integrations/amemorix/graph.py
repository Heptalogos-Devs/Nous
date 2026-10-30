"""Graph search projects real entities and relations under the cognition source owner."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.amemorix.contracts import GraphSearchInput
from nous_sdk.contracts import (
    AssociationBody,
    CallContext,
    CognitiveRef,
    DomainError,
    EntityBody,
)
from pydantic import JsonValue

from .helpers import json_object

if TYPE_CHECKING:
    from .service import AMemorixService


def match(fields: tuple[tuple[str, str], ...], query: str) -> tuple[str, str, int] | None:
    """Adapt A-Memorix's exact, prefix, then substring match priority."""
    best: tuple[str, str, int] | None = None
    for field, value in fields:
        token = value.strip().lower()
        rank = (
            0 if token == query else 1 if token.startswith(query) else 2 if query in token else None
        )
        if token and rank is not None and (best is None or rank < best[2]):
            best = (field, value, rank)
    return best


async def graph_admin(
    self: AMemorixService, ctx: CallContext, arguments: dict[str, JsonValue]
) -> dict[str, JsonValue]:
    if arguments.get("action") != "search":
        from .graph_actions import graph_action

        return await graph_action(self, ctx, arguments)
    command = GraphSearchInput.model_validate(arguments)
    query = command.query.strip()
    if not query:
        return {
            "success": False,
            "query": query,
            "limit": command.limit,
            "count": 0,
            "items": [],
            "error": "query must not be empty",
        }
    records = await self.records(ctx, ("entity", "association"))
    records = tuple(record for record in records if not record.deleted)
    known = {record.reference.exact(): record for record in records}

    async def endpoint(reference: CognitiveRef) -> str:
        record = known.get(reference.exact())
        if record is None:
            record = await self.app.cognition.get(ctx, reference)
            known[reference.exact()] = record
        return record.body.label if isinstance(record.body, EntityBody) else reference.exact()

    matches: list[tuple[int, int, str, str, dict[str, JsonValue]]] = []
    for record in records:
        body = record.body
        if isinstance(body, EntityBody):
            found = match((("name", body.label), ("hash", record.reference.id)), query.lower())
            if found:
                field, value, rank = found
                payload = json_object(
                    {
                        "type": "entity",
                        "title": body.label,
                        "matched_field": field,
                        "matched_value": value,
                        "entity_name": body.label,
                        "entity_hash": record.reference.id,
                        "reference": record.reference,
                        "source_roots": record.source_roots,
                    }
                )
                matches.append((rank, 0, body.label.lower(), record.reference.id, payload))
        elif isinstance(body, AssociationBody):
            try:
                subject, object_ = await endpoint(body.from_ref), await endpoint(body.to_ref)
            except DomainError as error:
                if error.code not in {"source_unavailable", "cognition_not_found"}:
                    raise
                continue
            found = match(
                (
                    ("subject", subject),
                    ("object", object_),
                    ("predicate", body.relation),
                    ("hash", record.reference.id),
                ),
                query.lower(),
            )
            if found:
                field, value, rank = found
                payload = json_object(
                    {
                        "type": "relation",
                        "title": f"{subject} — {body.relation} → {object_}",
                        "matched_field": field,
                        "matched_value": value,
                        "subject": subject,
                        "predicate": body.relation,
                        "object": object_,
                        "relation_hash": record.reference.id,
                        "reference": record.reference,
                        "source_roots": record.source_roots,
                        "polarity": body.polarity,
                    }
                )
                if body.epistemic is not None and body.epistemic.confidence is not None:
                    payload["confidence"] = body.epistemic.confidence
                matches.append((rank, 1, subject.lower(), record.reference.id, payload))
    matches.sort(key=lambda item: item[:4])
    selected = matches[: command.limit]
    # Recheck every selected exact revision before returning its current projection.
    for _, _, _, _, payload in selected:
        reference = CognitiveRef.model_validate(payload["reference"])
        await self.app.cognition.get(ctx, reference)
    return {
        "success": True,
        "query": query,
        "limit": command.limit,
        "count": len(selected),
        "items": [payload for _, _, _, _, payload in selected],
        "total_matches": len(matches),
    }
