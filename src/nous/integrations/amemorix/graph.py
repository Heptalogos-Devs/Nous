"""A-Memorix graph responses project the native typed graph search result."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.amemorix.contracts import GraphSearchInput
from nous_sdk.contracts import AssociationBody, CallContext, EntityBody
from nous_sdk.contracts.graph_management import GraphSearchQuery
from pydantic import JsonValue

from .helpers import json_object

if TYPE_CHECKING:
    from .service import AMemorixService


async def graph_admin(
    self: AMemorixService, ctx: CallContext, arguments: dict[str, JsonValue]
) -> dict[str, JsonValue]:
    if arguments.get("action") != "search":
        from .graph_actions import graph_action

        return await graph_action(self, ctx, arguments)
    command = GraphSearchInput.model_validate(arguments)
    if not command.query.strip():
        return {
            "success": False,
            "query": "",
            "limit": command.limit,
            "count": 0,
            "items": [],
            "error": "query must not be empty",
        }
    result = await self.app.graphs.search_graph(
        ctx, GraphSearchQuery(text=command.query, limit=command.limit)
    )
    items: list[dict[str, JsonValue]] = []
    for match in result.matches:
        record, body = match.record, match.record.body
        common = {
            "matched_field": "hash" if match.field == "id" else match.field,
            "matched_value": match.value,
            "reference": record.reference,
            "source_roots": record.source_roots,
        }
        if isinstance(body, EntityBody):
            items.append(
                json_object(
                    {
                        **common,
                        "type": "entity",
                        "title": body.label,
                        "entity_name": body.label,
                        "entity_hash": record.reference.id,
                    }
                )
            )
        elif isinstance(body, AssociationBody):
            item = json_object(
                {
                    **common,
                    "type": "relation",
                    "title": f"{match.source_name} — {body.relation} → {match.target_name}",
                    "subject": match.source_name,
                    "predicate": body.relation,
                    "object": match.target_name,
                    "relation_hash": record.reference.id,
                    "polarity": body.polarity,
                }
            )
            if body.epistemic is not None and body.epistemic.confidence is not None:
                item["confidence"] = body.epistemic.confidence
            items.append(item)
    return json_object(
        {
            "success": True,
            "query": result.query,
            "limit": command.limit,
            "count": len(items),
            "items": items,
            "total_matches": result.total_matches,
        }
    )
