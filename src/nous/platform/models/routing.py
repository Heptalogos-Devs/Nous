"""Compile one model request and execute it through the configured provider."""

from __future__ import annotations

from typing import TYPE_CHECKING, cast

from nous_sdk.contracts import CallContext, DomainError, EmbeddingBatch, MediaEmbeddingInput
from nous_sdk.contracts.model_roles import ModelCachePolicy
from nous_sdk.contracts.prompt_compilation import PromptMessage
from pydantic import TypeAdapter
from typing_extensions import TypeForm

from nous.platform.configuration import EmbeddingRoute
from nous.platform.errors import Unavailable

from .embedding import MediaResolver, embed_media as request_media
from .embedding_profile import EmbeddingProfile
from .generation import MediaInput, ModelResult, PreparedGeneration
from .text_embedding import embed_text

if TYPE_CHECKING:
    from . import ModelService


def required_modalities(media: tuple[MediaInput, ...]) -> set[str]:
    required = {"text"}
    for part in media:
        for modality in ("image", "audio", "video"):
            if part.media_type.startswith(modality + "/"):
                required.add(modality)
    return required


def ensure_capabilities(role: str, route, model, media: tuple[MediaInput, ...]) -> None:
    required = required_modalities(media)
    missing = sorted(required.difference(model.input_modalities))
    if missing:
        raise DomainError(
            "model_unavailable",
            "The selected model does not support this request's input modalities",
            role=role,
            reason="capability_missing",
            required=sorted(required),
            missing=missing,
        )
    if any(part.media_type.startswith("video/") for part in media) and route.protocol != "volcengine-ark":
        raise DomainError(
            "model_unavailable",
            "The selected API dialect cannot send native video",
            role=role,
            reason="video_dialect_unsupported",
        )


async def prepare_generation[T](
    self: ModelService,
    ctx: CallContext,
    role_id: str,
    variables: dict[str, object],
    output_type: TypeForm[T] | None = None,
    *,
    source_ids: tuple[str, ...] = (),
    media: tuple[MediaInput, ...] = (),
    context_messages: tuple[PromptMessage, ...] = (),
    max_output_tokens: int | None = None,
    cache_policy: ModelCachePolicy = "none",
    preset_index: int = 0,
) -> PreparedGeneration[T]:
    prepared = await self.prepare_role(
        ctx,
        role_id,
        variables,
        output_type=output_type,
        require_configured=False,
        context_messages=context_messages,
        max_output_tokens=max_output_tokens,
    )
    actual_output = output_type or cast(TypeForm[T], prepared.definition.output_type)
    if preset_index < 0 or preset_index >= len(prepared.routes):
        raise DomainError("model_unavailable", "The model role has no selected preset", role=role_id)
    candidate = prepared.routes[preset_index]
    ensure_capabilities(role_id, candidate.route, candidate.model, media)
    return self.generation.prepare(
        prepared.definition.template_id,
        candidate.route,
        prepared.prompt,
        actual_output,
        media,
        source_ids,
        cache_policy,
    )


async def execute_generation[T](
    self: ModelService, ctx: CallContext, prepared: PreparedGeneration[T]
) -> ModelResult[T]:
    from . import credential

    key = credential(prepared.route, prepared.task)
    return await self.generation.execute(ctx, prepared, key)


async def generate[T](
    self: ModelService,
    ctx: CallContext,
    role_id: str,
    variables: dict[str, object],
    output_type: TypeForm[T],
    *,
    source_ids: tuple[str, ...] = (),
    cache_policy: ModelCachePolicy = "none",
    media: tuple[MediaInput, ...] = (),
    context_messages: tuple[PromptMessage, ...] = (),
    max_output_tokens: int | None = None,
    allow_fallback: bool = False,
) -> ModelResult[T]:
    prepared_role = await self.prepare_role(
        ctx,
        role_id,
        variables,
        output_type=output_type,
        require_configured=False,
        context_messages=context_messages,
        max_output_tokens=max_output_tokens,
    )
    count = len(prepared_role.routes) if allow_fallback else min(1, len(prepared_role.routes))
    if count == 0:
        raise DomainError("model_unavailable", "The model role has no configured preset", role=role_id)
    last_error: Exception | None = None
    for index in range(count):
        try:
            candidate = prepared_role.routes[index]
            ensure_capabilities(role_id, candidate.route, candidate.model, media)
            request = self.generation.prepare(
                prepared_role.definition.template_id,
                candidate.route,
                prepared_role.prompt,
                output_type,
                media,
                source_ids,
                cache_policy,
            )
            return await self.execute_generation(ctx, request)
        except (DomainError, Unavailable) as error:
            last_error = error
            if not allow_fallback:
                raise
    assert last_error is not None
    raise last_error


async def embed(
    self: ModelService,
    ctx: CallContext,
    texts: tuple[str, ...],
    *,
    source_ids: tuple[str, ...] = (),
    profile: EmbeddingProfile | None = None,
    cache_policy: ModelCachePolicy | None = None,
    allow_fallback: bool = False,
) -> ModelResult[EmbeddingBatch]:
    return await _embed(
        self,
        ctx,
        texts=texts,
        media=None,
        source_ids=source_ids,
        profile=profile,
        cache_policy=cache_policy,
        allow_fallback=allow_fallback,
    )


async def embed_media(
    self: ModelService,
    ctx: CallContext,
    inputs: tuple[tuple[str | MediaEmbeddingInput, ...], ...],
    *,
    source_ids: tuple[str, ...] = (),
    profile: EmbeddingProfile | None = None,
    cache_policy: ModelCachePolicy | None = None,
    allow_fallback: bool = False,
) -> ModelResult[EmbeddingBatch]:
    return await _embed(
        self,
        ctx,
        texts=None,
        media=inputs,
        source_ids=source_ids,
        profile=profile,
        cache_policy=cache_policy,
        allow_fallback=allow_fallback,
    )


async def _embed(
    self: ModelService,
    ctx: CallContext,
    *,
    texts: tuple[str, ...] | None,
    media: tuple[tuple[str | MediaEmbeddingInput, ...], ...] | None,
    source_ids: tuple[str, ...],
    profile: EmbeddingProfile | None,
    cache_policy: ModelCachePolicy | None,
    allow_fallback: bool,
) -> ModelResult[EmbeddingBatch]:
    from . import credential

    selected = profile or await self.prepare_embedding(ctx)
    policy = cache_policy or selected.cache_policy
    routes = (selected.route, *selected.fallbacks) if allow_fallback else (selected.route,)
    last_error: Exception | None = None
    for route in routes:
        try:
            key = credential(route, "embedding")
            if texts is not None:
                return await embed_text(
                    ctx,
                    texts,
                    route,
                    key,
                    self.network,
                    self.generation,
                    source_ids,
                    cache_policy=policy,
                )
            assert media is not None
            if self.media_resolver is None:
                raise DomainError("media_unavailable", "The model media resolver is unavailable")
            return await request_media(
                ctx,
                media,
                route,
                key,
                self.media_resolver,
                self.network,
                self.generation,
                source_ids,
                cache_policy=policy,
            )
        except (DomainError, Unavailable) as error:
            last_error = error
            if not allow_fallback:
                raise
    assert last_error is not None
    raise last_error
