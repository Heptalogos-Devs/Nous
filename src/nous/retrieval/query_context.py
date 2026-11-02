"""Read optional query context without interpreting or rewriting natural language."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, QueryRequest
from nous_sdk.contracts.targets import MediaTarget
from pydantic import JsonValue

if TYPE_CHECKING:
    from .service import RetrievalService


async def resolve_context(
    self: RetrievalService,
    ctx: CallContext,
    request: QueryRequest,
) -> tuple[QueryRequest, dict[str, JsonValue]]:
    if request.context is None:
        return request, {"original_query": request.text}
    async with self.authority.read(ctx) as view:
        context = (
            await self.references.query_context_in(view, ctx, request.context)
            if isinstance(request.context, str)
            else request.context
        )
        bindings: list[JsonValue] = []
        anchors: list[str] = []
        entities: list[str] = []
        attachments = list(context.attachments)
        for locators, collected in ((context.anchors, anchors), (context.entities, entities)):
            for locator in locators:
                # Literal names remain contextual text. Only supplied addresses
                # request an identity lookup in the authorized reference directory.
                if ":" not in locator:
                    collected.append(locator)
                    continue
                target = (
                    await self.references.resolve_in(view, ctx, locator)
                ).reference
                collected.append(target.reference.exact())
                bindings.append({"input": locator, "reference": target.reference.exact()})
        for reference in attachments:
            await self.references.resolve_target_in(
                view, ctx, MediaTarget(reference=reference), None
            )
    context = context.model_copy(
        update={
            "anchors": tuple(anchors),
            "entities": tuple(entities),
            "attachments": tuple(dict.fromkeys(attachments)),
        }
    )
    return (
        request.model_copy(update={"context": context}),
        {
            "original_query": request.text,
            "bindings": bindings,
            "goal": context.goal,
            "reference_time": context.reference_time.model_dump(mode="json")
            if context.reference_time
            else None,
            "anchors": list(context.anchors),
            "entities": list(context.entities),
            "attachments": [item.exact() for item in context.attachments],
        },
    )
