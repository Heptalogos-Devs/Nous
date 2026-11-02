"""Language tasks for forming cognition from original sources."""

from __future__ import annotations

from nous_sdk.contracts.formation import (
    GraphDraft,
    GraphResolutionDraft,
    GraphResolutionInput,
    NotesDraft,
)
from nous_sdk.contracts.model_roles import ModelCachePolicy
from nous_sdk.contracts.namespace_policy import FormationPolicy

from nous.cognition.models import ModelServices
from nous.platform.call_context import CallContext
from nous.platform.configuration.service import ConfigurationService
from nous.platform.errors import DomainError
from nous.platform.models.generation import MediaInput, ModelResult


async def require_formation(
    configuration: ConfigurationService, ctx: CallContext, text: str
) -> FormationPolicy:
    policy = await configuration.policy(ctx, "formation", FormationPolicy)
    if not policy.enabled:
        raise DomainError("formation_disabled", "Namespace semantic formation is disabled")
    if len(text) > policy.max_chunk_chars:
        raise DomainError(
            "formation_chunk_limit", "Source exceeds the namespace formation chunk limit"
        )
    return policy


async def form_notes(
    models: ModelServices,
    ctx: CallContext,
    text: str,
    *,
    configuration: ConfigurationService,
    background: str = "",
    previous_memories: str = "",
    sources_labelled: bool = False,
    source_ids: tuple[str, ...] = (),
    cache_policy: ModelCachePolicy = "none",
    media: tuple[MediaInput, ...] = (),
) -> ModelResult[NotesDraft]:
    await require_formation(configuration, ctx, text)
    return await models.generate(
        ctx,
        "formation",
        {
            "source_text": text if sources_labelled else "[s1]\n" + text,
            "background": background,
            "previous_memories": previous_memories,
        },
        NotesDraft,
        source_ids=source_ids,
        cache_policy=cache_policy,
        media=media,
    )


async def extract_graph(
    models: ModelServices,
    ctx: CallContext,
    text: str,
    *,
    configuration: ConfigurationService,
    background: str = "",
    source_ids: tuple[str, ...] = (),
    cache_policy: ModelCachePolicy = "none",
    media: tuple[MediaInput, ...] = (),
) -> ModelResult[GraphDraft]:
    policy = await require_formation(configuration, ctx, text)
    result = await models.generate(
        ctx,
        "formation",
        {"source_text": text, "background": background},
        GraphDraft,
        source_ids=source_ids,
        cache_policy=cache_policy,
        media=media,
    )
    if policy.max_entities is not None and len(result.output.entities) > policy.max_entities:
        raise DomainError("formation_entity_limit", "Graph extraction exceeded the entity limit")
    if policy.max_relations is not None and len(result.output.relations) > policy.max_relations:
        raise DomainError(
            "formation_relation_limit", "Graph extraction exceeded the relation limit"
        )
    return result


async def resolve_graph(
    models: ModelServices,
    ctx: CallContext,
    request: GraphResolutionInput,
    *,
    source_ids: tuple[str, ...] = (),
) -> ModelResult[GraphResolutionDraft]:
    return await models.generate(
        ctx,
        "formation",
        request.model_dump(mode="json"),
        GraphResolutionDraft,
        source_ids=source_ids,
    )


async def describe(
    models: ModelServices,
    ctx: CallContext,
    text: str,
    *,
    background: str = "",
    source_ids: tuple[str, ...] = (),
    media: tuple[MediaInput, ...],
) -> ModelResult[str]:
    return await models.generate(
        ctx,
        "description",
        {"source_text": text, "background": background},
        str,
        source_ids=source_ids,
        media=media,
    )
