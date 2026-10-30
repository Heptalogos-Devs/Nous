"""Source formation uses the generic model owner and its registered invocation contract."""

import json
from collections.abc import Awaitable, Callable
from typing import Literal

from nous_sdk.contracts import CallContext, FormationProposal
from nous_sdk.contracts.cognition import FormationDraft
from nous_sdk.contracts.model_roles import ModelCachePolicy

from nous.cognition.models import ModelServices
from nous.platform.models.generation import MediaInput
from nous.platform.time import parse_timepoint

from .model_contracts import Description, MediaFormationDraft


async def form(
    models: ModelServices,
    ctx: CallContext,
    text: str,
    source: dict[str, object],
    *,
    invocation_id: str | None = None,
    cache_policy: ModelCachePolicy = "shared",
) -> FormationProposal:
    generated = await models.generate(
        ctx,
        "formation",
        {"source_text": text, "source": source},
        FormationDraft,
        invocation_id=invocation_id,
        cache_policy=cache_policy,
    )
    proposal = FormationProposal.model_validate(generated.output.model_dump(mode="json"))
    event = parse_timepoint(proposal.event_expression) if proposal.event_expression else None
    return proposal.model_copy(update={"producer_call_id": generated.call_id, "event": event})


async def interpret(
    models: ModelServices,
    ctx: CallContext,
    text: str,
    source: dict[str, object],
    media: tuple[MediaInput, ...],
    strategy: Literal["description_only", "direct_structured", "describe_then_structure"],
    *,
    description_adopter: Callable[[Description], Awaitable[None]] | None = None,
    cached_description: Description | None = None,
) -> Description | FormationProposal:
    variables: dict[str, object] = {
        "source_text": text,
        "source": source,
        "source_context": text + "\n" + json.dumps(source, ensure_ascii=False),
    }
    description = cached_description
    if strategy != "direct_structured" and description is None:
        generated = await models.generate(ctx, "description", variables, str, media=media)
        description = Description(description=generated.output, producer_call_id=generated.call_id)
        if description_adopter is not None:
            await description_adopter(description)
    if strategy == "description_only":
        assert description is not None
        return description
    if description is not None:
        variables["saved_description"] = description.model_dump(mode="json")
    formed = await models.generate(ctx, "formation", variables, MediaFormationDraft, media=media)
    result = FormationProposal.model_validate(formed.output.model_dump(mode="json"))
    event = parse_timepoint(result.event_expression) if result.event_expression else None
    return result.model_copy(update={"producer_call_id": formed.call_id, "event": event})
