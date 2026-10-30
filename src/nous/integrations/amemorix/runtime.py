"""Runtime administration reads and changes the configuration owner's records."""

from typing import TYPE_CHECKING
from uuid import uuid4

from nous_sdk.contracts import CallContext, Contract, DomainError
from pydantic import JsonValue

from .helpers import json_object

if TYPE_CHECKING:
    from .service import AMemorixService


class RuntimeInput(Contract):
    action: str
    value: JsonValue = None
    operation_id: str | None = None


async def runtime_admin(
    service: AMemorixService, ctx: CallContext, arguments: dict[str, JsonValue]
) -> dict[str, JsonValue]:
    command = RuntimeInput.model_validate(arguments)
    if command.action in {"get_config", "list_vector_spaces"}:
        from .diagnostics import diagnostics

        return await diagnostics(service, ctx)
    if command.action == "set_config":
        return json_object(
            {
                "success": True,
                "config": await service.app.configuration.set(
                    ctx, "models", command.value, command.operation_id or str(uuid4()), "subject"
                ),
            }
        )
    raise DomainError("capability_unavailable", "Runtime action has no configured owner")
