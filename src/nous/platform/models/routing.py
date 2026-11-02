"""Accepted route directories and physical intents share the authoritative call ledger."""

from __future__ import annotations

from hashlib import sha256
from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, DomainError, EmbeddingBatch, MediaEmbeddingInput
from nous_sdk.contracts.common import Contract
from nous_sdk.contracts.model_roles import ModelCachePolicy
from nous_sdk.contracts.model_routing import ModelCallOutcome, ModelRouteAttempt, ModelRoutingPolicy
from nous_sdk.contracts.prompt_compilation import CompiledPrompt, PromptMessage
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select
from typing_extensions import TypeForm

from nous.platform.configuration import ModelRoute
from nous.platform.configuration.models import ModelConfiguration
from nous.platform.errors import NousError, Unavailable
from nous.platform.persistence import OperationConflict, OperationResultPurged
from nous.platform.persistence.authority import operation_key
from nous.platform.persistence.schema import operations

from .contracts import PreparedRole
from .embedding import embed_media as request_media
from .embedding_profile import EmbeddingProfile
from .generation import Generated, MediaInput, prepare_invocation
from .invocations import child
from .outcomes import may_continue
from .retries import needs_provider_request, rate_limited
from .roles import json_object
from .text_embedding import embed_text

if TYPE_CHECKING:
    from . import ModelService


class GenerationRoute(Contract):
    id: str
    index: int
    route: ModelRoute
    capabilities: tuple[str, ...]
    invocation: dict[str, JsonValue] | None = None


class GenerationPlan(Contract):
    role: str
    purpose: str
    prompt: CompiledPrompt
    routes: tuple[GenerationRoute, ...]
    policy: ModelRoutingPolicy
    source_ids: tuple[str, ...]
    contract_purpose: str | None = None
    variant: str | None = None


async def saved_generation(
    self: ModelService, ctx: CallContext, role: str, intent: str
) -> tuple[GenerationPlan, dict[str, JsonValue] | None] | None:
    async with self.ledger.authority.read(ctx) as view:
        raw = (
            await view.connection.execute(
                select(operations.c.result_json, operations.c.payload_json).where(
                    operations.c.operation_key
                    == operation_key(ctx, "model.generation_plan", role, intent)
                )
            )
        ).first()
    if raw is None:
        return None
    result = TypeAdapter(JsonValue).validate_json(raw.result_json)
    if isinstance(result, dict) and result.get("$nous_operation_result") == "purged":
        raise OperationResultPurged(intent)
    return GenerationPlan.model_validate(result), TypeAdapter(dict[str, JsonValue]).validate_json(
        raw.payload_json
    ) if raw.payload_json else None


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


def has_sent_attempt(failures: list[dict[str, object]]) -> bool:
    for failure in failures:
        if not failure.get("call_id"):
            continue
        value = failure.get("outcome")
        if not isinstance(value, dict) or ModelCallOutcome.model_validate(value).kind != "not_sent":
            return True
    return False


async def generation_plan[T](
    self: ModelService,
    ctx: CallContext,
    role: str,
    variables: dict[str, object],
    output: TypeForm[T],
    source_ids: tuple[str, ...],
    media: tuple[MediaInput, ...],
    intent: str,
    cache_policy: ModelCachePolicy,
    prepared: PreparedRole | None = None,
    context_messages: tuple[PromptMessage, ...] = (),
    max_output_tokens: int | None = None,
) -> GenerationPlan:
    payload: dict[str, object] = {
        "max_output_tokens": max_output_tokens,
        "context_messages": [message.model_dump(mode="json") for message in context_messages],
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
        async with self.ledger.authority.write(
            ctx, intent, operation_kind="model.generation_plan", scope=role, payload=payload
        ) as tx:
            if tx.replayed:
                return GenerationPlan.model_validate(tx.replay_result)
            prepared = prepared or await self.prepare_role(
                ctx,
                role,
                variables,
                output_type=output,
                require_configured=False,
                context_messages=context_messages,
                max_output_tokens=max_output_tokens,
            )
            plan = GenerationPlan(
                role=role,
                contract_purpose=prepared.definition.purpose,
                variant=prepared.definition.variant,
                purpose=prepared.definition.invocation_purpose or role,
                prompt=prepared.prompt,
                policy=prepared.role.failure_policy,
                source_ids=source_ids,
                routes=tuple(
                    GenerationRoute(
                        id=item.id,
                        index=item.index,
                        route=item.route,
                        capabilities=item.model.input_modalities,
                        invocation=prepare_invocation(
                            prepared.definition.invocation_purpose or role,
                            item.route,
                            prepared.prompt,
                            output,
                            media,
                            source_ids,
                        ),
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


async def generate[T](
    self: ModelService,
    ctx: CallContext,
    role_id: str,
    variables: dict[str, object],
    output_type: TypeForm[T],
    *,
    source_ids: tuple[str, ...] = (),
    invocation_id: str | None = None,
    cache_policy: ModelCachePolicy = "shared",
    media: tuple[MediaInput, ...] = (),
    context_messages: tuple[PromptMessage, ...] = (),
    max_output_tokens: int | None = None,
) -> Generated[T]:
    from . import credential

    intent = identity(ctx, invocation_id, "generation")
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
        context_messages=context_messages,
        max_output_tokens=max_output_tokens,
    )
    failures: list[dict[str, object]] = []
    for item in plan.routes:
        attempt_id = child(intent, item.index)
        actual = await self.ledger.attempt(ctx, role_id, attempt_id)
        if actual is not None and actual.status == "succeeded":
            retained = await self.ledger.recall(ctx, role_id, attempt_id)
            assert retained is not None
            return Generated(
                TypeAdapter(output_type).validate_python(retained.output), retained.call_id
            )
        if actual is not None and actual.status != "succeeded":
            rate_retry = (
                plan.policy.rate_limit_retries
                and actual.outcome is not None
                and actual.outcome.kind == "rejected"
                and actual.outcome.http_status == 429
            )
            if (
                may_continue(plan.policy, actual.outcome)
                and not rate_retry
                and (actual.outcome is None or actual.outcome.kind != "not_sent")
            ):
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
            if not rate_retry and not (
                actual.outcome is not None
                and actual.outcome.kind == "not_sent"
                and plan.policy.not_sent
            ):
                await self.ledger.recall(ctx, role_id, attempt_id)
        current_resources = ModelConfiguration.model_validate(
            (await self.configuration.effective(ctx, "models"))["value"]
        )
        current_role = current_resources.roles[role_id]
        allowed = tuple(
            current_resources.resolve(role_id, index, require_configured=False)[0]
            for index in range(len(current_role.preset_ids()))
        )
        budget_route = next(
            (
                candidate
                for candidate in allowed
                if candidate.budget_resource == item.route.budget_resource
            ),
            None,
        )
        if budget_route is None:
            raise DomainError(
                "model_resource_unavailable",
                "The accepted model resource is no longer enabled for this task",
                role=role_id,
            )
        actual_route = item.route.model_copy(
            update={
                "max_calls_per_day": budget_route.max_calls_per_day,
                "resource_request_budget": budget_route.resource_request_budget,
            }
        )
        try:
            key = (
                credential(item.route, role_id)
                if await needs_provider_request(
                    ctx, role_id, (attempt_id,), plan.policy, self.ledger
                )
                else ""
            )
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

            async def request(
                identity: str | None,
                item: GenerationRoute = item,
                key: str = key,
                actual_route: ModelRoute = actual_route,
            ) -> tuple[T, str]:
                return await self.generation.request(
                    ctx,
                    plan.purpose,
                    actual_route,
                    key,
                    plan.prompt,
                    output_type,
                    media,
                    source_ids=source_ids,
                    budget_role=role_id,
                    invocation_id=identity,
                    cache_policy=cache_policy,
                    routing=ModelRouteAttempt(
                        intent_id=intent, route_id=item.id, route_index=item.index
                    ),
                    request_snapshot=item.invocation,
                )

            result, call_id = await rate_limited(
                ctx, role_id, attempt_id, plan.policy, self.ledger, request
            )
        except DomainError as error:
            failed_identity = error.details.get("invocation_id")
            if error.code == "model_invocation_unavailable" and isinstance(failed_identity, str):
                actual = await self.ledger.attempt(ctx, role_id, failed_identity)
                if actual is not None and may_continue(plan.policy, actual.outcome):
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
            if error.code != "model_unavailable" or not plan.policy.unavailable:
                raise
            failures.append({"route_id": item.id, "reason": error.details})
            continue
        except Unavailable as error:
            failed_identity = error.context.get("invocation_id")
            actual = await self.ledger.attempt(
                ctx, role_id, failed_identity if isinstance(failed_identity, str) else attempt_id
            )
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
    if not has_sent_attempt(failures):
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
            # A persisted profile keeps the exact fields present when accepted.
            # Newly introduced defaults must not alter an existing intent's hash.
            payload={
                "input": payload,
                "permissions": permissions(ctx),
                "profile": profile.model_dump(mode="json", exclude_unset=True)
                if profile is not None
                else None,
            },
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
    retry_authorization: str | None = None,
) -> EmbeddingBatch:
    return await route_embedding(
        self, ctx, texts, None, source_ids, invocation_id, profile, retry_authorization
    )


async def embed_media(
    self: ModelService,
    ctx: CallContext,
    inputs: tuple[tuple[str | MediaEmbeddingInput, ...], ...],
    *,
    source_ids: tuple[str, ...] = (),
    invocation_id: str | None = None,
    profile: EmbeddingProfile | None = None,
    retry_authorization: str | None = None,
) -> EmbeddingBatch:
    return await route_embedding(
        self, ctx, None, inputs, source_ids, invocation_id, profile, retry_authorization
    )


async def route_embedding(
    self: ModelService,
    ctx: CallContext,
    texts: tuple[str, ...] | None,
    media: tuple[tuple[str | MediaEmbeddingInput, ...], ...] | None,
    source_ids: tuple[str, ...],
    invocation_id: str | None,
    profile: EmbeddingProfile | None,
    retry_authorization: str | None = None,
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
            physical_ids = tuple(
                f"{prefix}:{index}"
                for index in range(
                    (len(texts) + route.max_batch_items - 1) // route.max_batch_items
                    if texts is not None
                    else len(media or ())
                )
            )
            key = (
                credential(route, "embedding")
                if await needs_provider_request(
                    ctx,
                    "embedding",
                    physical_ids,
                    accepted.fallback_policy,
                    self.ledger,
                    authorization=retry_authorization,
                )
                else ""
            )
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
                    retry_policy=accepted.fallback_policy,
                    cache_policy=accepted.cache_policy,
                    retry_authorization=retry_authorization,
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
                retry_policy=accepted.fallback_policy,
                cache_policy=accepted.cache_policy,
                retry_authorization=retry_authorization,
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
    if not has_sent_attempt(failures):
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
