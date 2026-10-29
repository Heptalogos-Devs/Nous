"""MaiBot profile browsing consumes the same retained person and evidence owner."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.amemorix.contracts import ProfileAdminInput
from nous_wave.contracts import CallContext, DomainError
from pydantic import JsonValue

from .helpers import json_object
from .profile_read import aliases, people, read_profile, require_retained, select_person, summary

if TYPE_CHECKING:
    from .service import AMemorixService


async def profile_admin(
    self: AMemorixService, ctx: CallContext, arguments: dict[str, JsonValue]
) -> dict[str, JsonValue]:
    if arguments.get("action") not in {"list", "query", "evidence", "get_aliases"}:
        raise DomainError(
            "capability_unavailable",
            "Profile action requires its native person operation",
            action=arguments.get("action"),
        )
    command = ProfileAdminInput.model_validate(arguments)
    if command.action == "list" and "limit" not in arguments and "top_k" not in arguments:
        command = command.model_copy(update={"limit": 50})
    require_retained(command)
    if command.action == "list":
        items = [summary(record) for record in (await people(self, ctx))[: command.limit]]
        return json_object({"success": True, "items": items, "count": len(items)})
    record = await select_person(self, ctx, command)
    if command.action == "get_aliases":
        return json_object(await aliases(self, ctx, record))
    result = await read_profile(self, ctx, record, command)
    if command.action == "query":
        result["profile"] = summary(record)
    return json_object(result)
