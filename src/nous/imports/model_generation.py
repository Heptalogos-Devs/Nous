"""Source formation uses the generic model owner and its registered invocation contract."""

import json
from collections.abc import Awaitable, Callable
from typing import Literal

from nous_sdk.contracts import CallContext, DomainError, FormationProposal
from nous_sdk.contracts.cognition import FormationDraft
from nous_sdk.contracts.model_roles import ModelCachePolicy
from nous_sdk.contracts.namespace_policy import FormationPolicy

from nous.cognition.formation_validation import validate_formation_policy
from nous.cognition.models import ModelServices
from nous.platform.configuration.service import ConfigurationService
from nous.platform.models.generation import MediaInput
from nous.platform.time import parse_timepoint

from .model_contracts import Description, MediaFormationDraft


async def form(
    models: ModelServices,
    ctx: CallContext,
    text: str,
    source: dict[str, object],
    *,
    configuration: ConfigurationService,
    invocation_id: str | None = None,
    cache_policy: ModelCachePolicy = "shared",
) -> FormationProposal:
    policy = await configuration.policy(ctx, "formation", FormationPolicy)
    if not policy.enabled:
        raise DomainError("formation_disabled", "Namespace semantic formation is disabled")
    if len(text) > policy.max_chunk_chars:
        raise DomainError(
            "formation_chunk_limit", "Source exceeds the namespace formation chunk limit"
        )
    source = {**source, "formation_policy": policy.model_dump(mode="json")}
    generated = await models.generate(
        ctx,
        "formation",
        {"source_text": text, "source": source},
        FormationDraft,
        invocation_id=invocation_id,
        cache_policy=cache_policy,
    )
    proposal = FormationProposal.model_validate(generated.output.model_dump(mode="json"))
    validate_formation_policy(proposal, policy)
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
    configuration: ConfigurationService,
    description_adopter: Callable[[Description], Awaitable[None]] | None = None,
    cached_description: Description | None = None,
) -> Description | FormationProposal:
    policy = await configuration.policy(ctx, "formation", FormationPolicy)
    if strategy != "description_only" and not policy.enabled:
        raise DomainError("formation_disabled", "Namespace semantic formation is disabled")
    source = {**source, "formation_policy": policy.model_dump(mode="json")}
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
    validate_formation_policy(result, policy)
    event = parse_timepoint(result.event_expression) if result.event_expression else None
    return result.model_copy(update={"producer_call_id": formed.call_id, "event": event})
