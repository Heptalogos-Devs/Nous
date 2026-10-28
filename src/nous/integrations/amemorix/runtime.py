"""Runtime administration reads and changes the configuration owner's records."""

from typing import TYPE_CHECKING
from uuid import uuid4

from nous_wave.contracts import CallContext, Contract, DomainError
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
    if command.action == "get_config":
        return json_object(
            {
                "success": True,
                "config": await service.app.configuration.effective(ctx, "models"),
                "integration": await service.policy(ctx),
            }
        )
    if command.action == "set_config":
        return json_object(
            {
                "success": True,
                "config": await service.app.configuration.set(
                    ctx, "models", command.value, command.operation_id or str(uuid4()), "subject"
                ),
            }
        )
    if command.action == "list_vector_spaces":
        from sqlalchemy import select

        from nous.cognition.schema import vectors

        async with service.app.authority.read(ctx) as view:
            readable = {record.reference.exact() for record in await service.records(ctx)}
            query = select(vectors.c.space, vectors.c.reference).where(
                vectors.c.reference.in_(readable)
            )
            counts: dict[str, int] = {}
            for space, _ in (await view.connection.execute(query)).all():
                counts[str(space)] = counts.get(str(space), 0) + 1
        return {
            "success": True,
            "spaces": [
                {"space": key, "vector_count": value} for key, value in sorted(counts.items())
            ],
        }
    raise DomainError("action_invalid", "Unknown runtime management action")
