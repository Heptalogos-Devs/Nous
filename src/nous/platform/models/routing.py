"""Accepted route directories and physical intents share the authoritative call ledger."""

from __future__ import annotations

from hashlib import sha256
from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, DomainError, EmbeddingBatch, MediaEmbeddingInput
from nous_sdk.contracts.common import Contract
from nous_sdk.contracts.model_roles import ModelCachePolicy
from nous_sdk.contracts.model_routing import ModelRouteAttempt, ModelRoutingPolicy
from pydantic import BaseModel, JsonValue, TypeAdapter

from nous.platform.configuration import ModelRoute
from nous.platform.errors import NousError, Unavailable
from nous.platform.persistence import OperationConflict

from .embedding import embed_media as request_media
from .embedding_profile import EmbeddingProfile
from .generation import Generated, MediaInput, prepare_invocation
from .invocations import InvocationAttempt
from .outcomes import may_continue
from .roles import json_object
from .text_embedding import embed_text

if TYPE_CHECKING:
    from . import ModelService


class GenerationRoute(Contract):
    id: str
    index: int
    route: ModelRoute
    capabilities: tuple[str, ...]


class GenerationPlan(Contract):
    role: str
    purpose: str
    prompt: dict[str, JsonValue]
    routes: tuple[GenerationRoute, ...]
    policy: ModelRoutingPolicy
    source_ids: tuple[str, ...]


def identity(ctx: CallContext, supplied: str | None, mechanism: str) -> str:
    return supplied or f"request:{mechanism}:" + sha256(ctx.request_id.encode()).hexdigest()


def permissions(ctx: CallContext) -> dict[str, JsonValue]:
    return json_object(
        {
            "credential_ceiling": sorted(ctx.credential_source_ids)
            if ctx.credential_source_ids is not None
            else None,
            "source_groups": sorted(sorted(set(group)) for group in ctx.source_groups()),
        }
    )


def child(identity: str, index: int) -> str:
    return f"route:{sha256(identity.encode()).hexdigest()}:{index}"


async def generation_plan[T: BaseModel | str](
    self: ModelService,
    ctx: CallContext,
    role: str,
    variables: dict[str, object],
    output: type[T],
    source_ids: tuple[str, ...],
    media: tuple[MediaInput, ...],
    intent: str,
    cache_policy: ModelCachePolicy,
    parent: InvocationAttempt | None,
) -> GenerationPlan | None:
    payload = {
        "role": role,
        "variables": variables,
        "schema": TypeAdapter(output).json_schema(),
        "source_ids": source_ids,
        "permissions": permissions(ctx),
        "cache_policy": cache_policy,
        "media": [
            (sha256(item.data).hexdigest(), len(item.data), item.media_type, item.source_reference)
            for item in media
        ],
    }
    try:
        if parent is not None:
            saved = await self.ledger.authority.receipt(
                ctx, intent, operation_kind="model.generation_plan", scope=role, payload=payload
            )
            if saved is not None:
                return GenerationPlan.model_validate(saved.result)
            # A physical parent already exists without a separate accepted route
            # directory. Prove its input before returning it; do not create a new
            # accepted plan which could replace that earlier private intent.
            prepared = await self.prepare_role(
                ctx, role, variables, output_type=output, require_configured=False
            )
            route = prepared.routes[0].route
            if parent.request.get("video_content_format") == "video_url":
                route = route.model_copy(update={"video_content_format": "video_url"})
            expected = prepare_invocation(
                prepared.definition.invocation_purpose or role,
                route,
                prepared.prompt,
                output,
                media,
                source_ids,
            )
            prior_prompt, current_prompt = parent.request.get("prompt"), expected.get("prompt")
            before = prior_prompt.get("variables") if isinstance(prior_prompt, dict) else None
            after = current_prompt.get("variables") if isinstance(current_prompt, dict) else None
            if before != after or any(
                parent.request.get(key) != expected.get(key)
                for key in ("schema", "media", "source_ids")
            ):
                raise DomainError(
                    "model_invocation_conflict",
                    "The retained parent has a different semantic input",
                )
            if ctx.source_groups():
                raise DomainError(
                    "model_invocation_scope_unavailable",
                    "The retained call has no accepted source-scope proof for this restricted caller",
                )
            return None
        async with self.ledger.authority.write(
            ctx, intent, operation_kind="model.generation_plan", scope=role, payload=payload
        ) as tx:
            if tx.replayed:
                return GenerationPlan.model_validate(tx.replay_result)
            prepared = await self.prepare_role(
                ctx, role, variables, output_type=output, require_configured=False
            )
            plan = GenerationPlan(
                role=role,
                purpose=prepared.definition.invocation_purpose or role,
                prompt=json_object(prepared.prompt),
                policy=prepared.role.fallback,
                source_ids=source_ids,
                routes=tuple(
                    GenerationRoute(
                        id=item.id,
                        index=item.index,
                        route=item.route,
                        capabilities=item.model.capabilities,
                    )
                    for item in prepared.routes
                ),
            )
            return tx.result(plan)
    except OperationConflict as error:
        raise DomainError(
            "model_invocation_conflict",
            "Accepted intent input or source permissions have changed",
            invocation_id=intent,
        ) from error


async def generate[T: BaseModel | str](
    self: ModelService,
    ctx: CallContext,
    role_id: str,
    variables: dict[str, object],
    output_type: type[T],
    *,
    source_ids: tuple[str, ...] = (),
    invocation_id: str | None = None,
    cache_policy: ModelCachePolicy = "shared",
    media: tuple[MediaInput, ...] = (),
) -> Generated[T]:
    from . import credential

    intent = identity(ctx, invocation_id, "generation")
    previous = await self.ledger.attempt(ctx, role_id, intent)
    plan = await generation_plan(
        self,
        ctx,
        role_id,
        variables,
        output_type,
        source_ids,
        media,
        intent,
        cache_policy,
        previous,
    )
    if previous is not None:
        retained = await self.ledger.recall(ctx, role_id, intent)
        assert retained is not None
        return Generated(
            TypeAdapter(output_type).validate_python(retained.output), retained.call_id
        )
    assert plan is not None
    failures: list[dict[str, object]] = []
    for item in plan.routes:
        attempt_id = child(intent, item.index)
        actual = await self.ledger.attempt(ctx, role_id, attempt_id)
        if actual is not None and actual.status != "succeeded":
            if may_continue(plan.policy, actual.outcome):
                failures.append(
                    {
                        "route_id": item.id,
                        "call_id": actual.call_id,
                        "outcome": actual.outcome.model_dump(mode="json")
                        if actual.outcome
                        else None,
                    }
                )
                continue
            await self.ledger.recall(ctx, role_id, attempt_id)
        try:
            key = credential(item.route, role_id)
            required = {
                "image"
                if part.media_type.startswith("image/")
                else "audio"
                if part.media_type.startswith("audio/")
                else "video"
                for part in media
                if part.media_type.startswith(("image/", "audio/", "video/"))
            }
            if not required.issubset(item.capabilities):
                raise DomainError(
                    "model_unavailable",
                    "Route lacks required media capabilities",
                    role=role_id,
                    reason="capability_missing",
                )
            result, call_id = await self.generation.request(
                ctx,
                plan.purpose,
                item.route,
                key,
                dict(plan.prompt),
                output_type,
                media,
                source_ids=source_ids,
                budget_role=role_id,
                invocation_id=attempt_id,
                cache_policy=cache_policy,
                routing=ModelRouteAttempt(
                    intent_id=intent, route_id=item.id, route_index=item.index
                ),
            )
        except DomainError as error:
            if error.code != "model_unavailable" or not plan.policy.unavailable:
                raise
            failures.append({"route_id": item.id, "reason": error.details})
            continue
        except Unavailable:
            actual = await self.ledger.attempt(ctx, role_id, attempt_id)
            if actual is None or not may_continue(plan.policy, actual.outcome):
                raise
            failures.append(
                {
                    "route_id": item.id,
                    "call_id": actual.call_id,
                    "outcome": actual.outcome.model_dump(mode="json") if actual.outcome else None,
                }
            )
            continue
        await self.ledger.bind_success(ctx, role_id, intent, call_id)
        return Generated(result, call_id)
    if not any(item.get("call_id") for item in failures):
        raise DomainError(
            "model_unavailable",
            "No configured route is locally available",
            role=role_id,
            routes=failures,
        )
    raise NousError(
        "model_routes_exhausted",
        "Every eligible route definitively rejected the request",
        status=503,
        context=json_object({"role": role_id, "attempts": failures}),
    )


async def embedding_plan(
    self: ModelService,
    ctx: CallContext,
    intent: str,
    payload: object,
    profile: EmbeddingProfile | None,
) -> EmbeddingProfile:
    try:
        async with self.ledger.authority.write(
            ctx,
            intent,
            operation_kind="model.embedding_plan",
            scope="embedding",
            payload={"input": payload, "permissions": permissions(ctx), "profile": profile},
        ) as tx:
            if tx.replayed:
                return EmbeddingProfile.model_validate(tx.replay_result)
            accepted = profile or await self.prepare_embedding(ctx)
            return tx.result(accepted)
    except OperationConflict as error:
        raise DomainError(
            "model_invocation_conflict",
            "Embedding intent input, profile or source permissions changed",
            invocation_id=intent,
        ) from error


async def embed(
    self: ModelService,
    ctx: CallContext,
    texts: tuple[str, ...],
    *,
    source_ids: tuple[str, ...] = (),
    invocation_id: str | None = None,
    profile: EmbeddingProfile | None = None,
) -> EmbeddingBatch:
    return await route_embedding(self, ctx, texts, None, source_ids, invocation_id, profile)


async def embed_media(
    self: ModelService,
    ctx: CallContext,
    inputs: tuple[tuple[str | MediaEmbeddingInput, ...], ...],
    *,
    source_ids: tuple[str, ...] = (),
    invocation_id: str | None = None,
    profile: EmbeddingProfile | None = None,
) -> EmbeddingBatch:
    return await route_embedding(self, ctx, None, inputs, source_ids, invocation_id, profile)


async def route_embedding(
    self: ModelService,
    ctx: CallContext,
    texts: tuple[str, ...] | None,
    media: tuple[tuple[str | MediaEmbeddingInput, ...], ...] | None,
    source_ids: tuple[str, ...],
    invocation_id: str | None,
    profile: EmbeddingProfile | None,
) -> EmbeddingBatch:
    from . import credential

    intent = identity(
        ctx, invocation_id, "text_embedding" if texts is not None else "media_embedding"
    )
    accepted = await embedding_plan(
        self, ctx, intent, {"texts": texts, "media": media, "source_ids": source_ids}, profile
    )
    failures: list[dict[str, object]] = []
    for index, route in enumerate((accepted.route, *accepted.fallbacks)):
        prefix = intent if index == 0 else child(intent, index)
        route_id = accepted.route_ids[index] if accepted.route_ids else None
        metadata = ModelRouteAttempt(intent_id=intent, route_id=route_id, route_index=index)
        try:
            key = credential(route, "embedding")
            if texts is not None:
                return await embed_text(
                    ctx,
                    texts,
                    route,
                    key,
                    self.ledger,
                    self.network,
                    source_ids,
                    invocation_id=prefix,
                    routing=metadata,
                )
            if self.media_resolver is None:
                raise DomainError(
                    "model_unavailable", "Media embedding resolver is unavailable", role="embedding"
                )
            assert media is not None
            return await request_media(
                ctx,
                media,
                route,
                key,
                self.media_resolver,
                self.ledger,
                self.network,
                source_ids,
                invocation_id=prefix,
                routing=metadata,
            )
        except DomainError as error:
            if error.code == "model_unavailable" and accepted.fallback_policy.unavailable:
                failures.append({"route_id": route_id, "reason": error.details})
                continue
            if error.code != "model_invocation_unavailable":
                raise
            call = error.details.get("invocation_id")
            actual = (
                await self.ledger.attempt(ctx, "embedding", call) if isinstance(call, str) else None
            )
            if actual is None or not may_continue(accepted.fallback_policy, actual.outcome):
                raise
            failures.append(
                {
                    "route_id": route_id,
                    "call_id": actual.call_id,
                    "outcome": actual.outcome.model_dump(mode="json") if actual.outcome else None,
                }
            )
        except Unavailable as error:
            call = error.context.get("invocation_id")
            actual = (
                await self.ledger.attempt(ctx, "embedding", call) if isinstance(call, str) else None
            )
            if actual is None or not may_continue(accepted.fallback_policy, actual.outcome):
                raise
            failures.append(
                {
                    "route_id": route_id,
                    "call_id": actual.call_id,
                    "outcome": actual.outcome.model_dump(mode="json") if actual.outcome else None,
                }
            )
    if not any(item.get("call_id") for item in failures):
        raise DomainError(
            "model_unavailable",
            "No embedding route is locally available",
            role="embedding",
            routes=failures,
        )
    raise NousError(
        "model_routes_exhausted",
        "Every embedding route definitively rejected the request",
        status=503,
        context=json_object({"role": "embedding", "attempts": failures}),
    )
