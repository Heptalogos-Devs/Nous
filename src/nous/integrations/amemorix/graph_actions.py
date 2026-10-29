"""Graph actions translate external names into the single cognition graph owner."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.contracts import AssociationBody, CallContext, Contract, DomainError, EntityBody
from nous_wave.contracts.graph_management import GraphChange
from pydantic import BaseModel, Field, JsonValue, ValidationError

from .graph_projection import details, edge, graph_view
from .helpers import json_object

if TYPE_CHECKING:
    from .service import AMemorixService


def parse[T: BaseModel](contract: type[T], value: object) -> T:
    try:
        return contract.model_validate(value)
    except ValidationError as error:
        raise DomainError(
            "InvalidArgument",
            "Invalid graph action fields",
            fields=[
                {"location": [str(part) for part in item["loc"]], "message": item["msg"]}
                for item in error.errors()
            ],
        ) from error


class ReadGraph(Contract):
    limit: int = Field(default=200, ge=1, le=1000)
    relation_limit: int = Field(default=20, ge=1, le=1000)
    paragraph_limit: int = Field(default=20, ge=1, le=1000)
    evidence_node_limit: int = Field(default=80, ge=1, le=1000)


def change(arguments: dict[str, JsonValue]) -> GraphChange:
    action = arguments.get("action")
    payload: dict[str, JsonValue] = {"action": action, "metadata": {"external_request": arguments}}
    for key in ("operation_id", "reference", "reason", "source_paragraph", "predicate", "weight"):
        if key in arguments:
            payload[key] = arguments[key]
    for destination, aliases in {
        "name": ("name", "old_name", "node", "hash_or_name"),
        "new_name": ("new_name", "target_name"),
        "subject": ("subject", "source"),
        "object": ("object", "target"),
        "relation_hash": ("relation_hash", "hash"),
        "reported_by": ("updated_by", "requested_by"),
    }.items():
        value = next(
            (arguments.get(alias) for alias in aliases if arguments.get(alias) is not None), None
        )
        if value is not None:
            payload[destination] = value
    if "predicate" not in payload and arguments.get("label") is not None:
        payload["predicate"] = arguments["label"]
    if action == "update_edge_weight" and "weight" not in payload:
        payload["weight"] = arguments.get("confidence")
    if action == "create_edge":
        payload["confidence"] = arguments.get("confidence", 1.0)
    return parse(GraphChange, payload)


async def graph_action(
    self: AMemorixService, ctx: CallContext, arguments: dict[str, JsonValue]
) -> dict[str, JsonValue]:
    action = arguments.get("action")
    if action in {"get_graph", "node_detail", "edge_detail"}:
        limits = parse(
            ReadGraph,
            {key: value for key, value in arguments.items() if key in ReadGraph.model_fields},
        )
        snapshot = await self.app.cognition.graph_snapshot(ctx)
        if action == "get_graph":
            return graph_view(snapshot, limits.limit)
        node = (
            arguments.get("node_id") or arguments.get("node") if action == "node_detail" else None
        )
        source = arguments.get("source")
        target = arguments.get("target") or arguments.get("object")
        return details(
            snapshot,
            node=str(node) if isinstance(node, str) else None,
            source=str(source) if isinstance(source, str) else None,
            target=str(target) if isinstance(target, str) else None,
            relation_limit=limits.relation_limit,
            paragraph_limit=limits.paragraph_limit,
            evidence_node_limit=limits.evidence_node_limit,
        )
    command = change(arguments)
    result = await self.app.cognition.manage_graph(ctx, command)
    entities = [row for row in result.records if isinstance(row.body, EntityBody)]
    associations = [row for row in result.records if isinstance(row.body, AssociationBody)]
    response = json_object(
        {
            "success": True,
            "operation": {
                "operation_id": result.operation_id,
                "commit_seq": result.commit_seq,
                "observation_id": result.observation_id,
                "report_material_id": result.report_material_id,
            },
            "records": result.records,
            "projection": {"status": "authority_updated"},
            "vector_projection": {
                "status": "not_requested",
                "retired_associations": result.retired_vector_associations,
            },
        }
    )
    if action == "create_node":
        value = entities[-1]
        assert isinstance(value.body, EntityBody)
        response.update(
            json_object(
                {
                    "created": value.reference in result.created,
                    "revived": value.reference in result.revived,
                    "node": {
                        "name": value.body.label,
                        "hash": value.reference.id,
                        "reference": value.reference,
                    },
                }
            )
        )
    elif action == "rename_node":
        value = entities[0]
        assert isinstance(value.body, EntityBody)
        previous = (
            await self.app.cognition.get(ctx, value.cause.parent)
            if value.cause and value.cause.parent
            else None
        )
        old_name = (
            previous.body.label if previous and isinstance(previous.body, EntityBody) else None
        )
        response.update(
            json_object(
                {
                    "renamed": old_name != value.body.label,
                    "old_name": old_name,
                    "new_name": value.body.label,
                    "entity_hash": value.reference.id,
                    "related_relation_hashes": [row.reference.id for row in associations],
                }
            )
        )
    elif action == "create_edge":
        known = {
            row.reference.id: row for row in result.records if isinstance(row.body, EntityBody)
        }
        value = associations[-1]
        response.update(
            json_object(
                {
                    "created": value.reference in result.created,
                    "reactivated": bool(result.revived),
                    "edge": edge(value, known),
                }
            )
        )
    elif action == "update_edge_weight":
        response.update(
            json_object(
                {
                    "updated": len(associations),
                    "weight": command.weight,
                    "relation_hashes": [row.reference.id for row in associations],
                }
            )
        )
    else:
        response.update(
            json_object(
                {
                    "deleted": bool(entities) if action == "delete_node" else len(associations),
                    "deleted_entity_count": len(entities),
                    "deleted_relation_count": len(associations),
                    "deleted_vector_count": result.retired_vector_associations,
                    "deleted_hashes": [row.reference.id for row in result.records if row.deleted],
                }
            )
        )
    return response
