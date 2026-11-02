"""Native provider SDKs share managed clients, prompts and the paid call ledger."""

import asyncio
import os
from collections.abc import Awaitable, Callable

from nous_sdk.contracts import (
    CallContext,
    DomainError,
    EmbeddingBatch,
    MediaReference,
)
from nous_sdk.contracts.model_measurement import ModelMeasurement
from nous_sdk.contracts.prompt_compilation import CompiledPrompt, PromptMessage
from pydantic import JsonValue, TypeAdapter

from nous.platform.configuration import EmbeddingRoute, ModelRoute, Settings
from nous.platform.configuration.models import ModelConfiguration
from nous.platform.configuration.service import ConfigurationService
from nous.platform.network.service import NetworkService
from nous.platform.persistence import Authority, ReadView, WriteTx
from nous.platform.prompts import PromptService

from . import routing
from .contracts import model_contract, model_preview, prepare_role
from .embedding import MediaResolver, retained_embedding
from .embedding_evidence import find_embedding_in, reusable_embedding_in
from .embedding_profile import EmbeddingProfile
from .embedding_spaces import configured_spaces_in
from .generation import (
    Generation,
    MediaInput,
)
from .ledger import CallLedger
from .retained_output import retained_output
from .roles import RoleRegistry

os.environ.setdefault("PYDANTIC_AI_NO_BANNER", "1")  # pyright: ignore[reportUnusedCallResult]


def credential(route: ModelRoute, role_id: str | None = None) -> str:
    if not route.model:
        raise DomainError(
            "model_unavailable",
            "Model ID has not been configured for this role",
            role=role_id,
            reason="model_missing",
        )
    try:
        value = (
            route.credential_file.read_text().strip()
            if route.credential_file
            else os.environ.get(route.credential_env, "")
        )
    except OSError as error:
        raise DomainError(
            "model_unavailable",
            "Configured model credential file is unavailable",
            role=role_id,
            reason="credential_unavailable",
        ) from error
    if not value:
        raise DomainError(
            "model_unavailable",
            "Model credential is not configured",
            role=role_id,
            reason="credential_missing",
        )
    return value


class ModelService:
    configured_embedding_spaces_in = configured_spaces_in
    retained_output = retained_output

    async def measure_host(
        self,
        ctx: CallContext,
        request: dict[str, JsonValue],
        *,
        context_window: int | None = None,
        output_reserve: int = 0,
    ) -> ModelMeasurement:
        from functools import partial

        import anyio

        from .measurement import measure

        resources = ModelConfiguration.model_validate(
            (await self.configuration.effective(ctx, "models"))["value"]
        )
        matches = tuple(
            model
            for model in resources.catalog.values()
            if model.actual_name and model.actual_name == request.get("model")
        )
        model = matches[0] if len(matches) == 1 else None
        return await anyio.to_thread.run_sync(
            partial(
                measure,
                request,
                model,
                context_window=context_window,
                output_reserve=output_reserve,
            )
        )

    async def measure_native(
        self, ctx: CallContext, role: str, request: dict[str, JsonValue]
    ) -> ModelMeasurement:
        from functools import partial

        import anyio

        from .measurement import measure

        resources = ModelConfiguration.model_validate(
            (await self.configuration.effective(ctx, "models"))["value"]
        )
        selected = resources.roles[role]
        model = resources.catalog[resources.presets[selected.preset].model]
        route, _ = resources.resolve(role, require_configured=False)
        payload = dict(request)
        prepared_media_tokens: int | None = 0
        if isinstance(payload.get("prompt"), dict):
            compiled = CompiledPrompt.model_validate(payload["prompt"])
            parameters = payload.get("parameters")
            declared = parameters.get("max_output_tokens") if isinstance(parameters, dict) else None
            reserve = int(declared) if isinstance(declared, int) else route.max_output_tokens
            if request.get("media"):
                prepared_media_tokens = compiled.token_estimate.media_tokens
            payload = TypeAdapter(dict[str, JsonValue]).validate_python(
                {
                    "model": model.actual_name,
                    "messages": [message.model_dump(mode="json") for message in compiled.messages],
                    "response_format": payload.get("provider_response_format"),
                    "max_output_tokens": reserve,
                }
            )
        else:
            declared = next(
                (
                    payload[name]
                    for name in ("max_output_tokens", "max_completion_tokens", "max_tokens")
                    if isinstance(payload.get(name), int)
                ),
                None,
            )
            reserve = int(declared) if isinstance(declared, int) else route.max_output_tokens
        measured = await anyio.to_thread.run_sync(
            partial(measure, {**payload, "model": model.actual_name}, model, output_reserve=reserve)
        )
        if request.get("media"):
            measured = measured.model_copy(
                update={
                    "input_tokens": measured.input_tokens + (prepared_media_tokens or 0),
                    "media_items": (
                        *measured.media_items,
                        {
                            "kind": "prepared_media",
                            "tokens": prepared_media_tokens,
                            "method": "compiled_media",
                        },
                    ),
                }
            )
        return measured

    async def accept_generation(
        self,
        ctx: CallContext,
        role: str,
        variables: dict[str, object],
        *,
        invocation_id: str,
        source_ids: tuple[str, ...] = (),
        media: tuple[MediaInput, ...] = (),
        context_messages: tuple[PromptMessage, ...] = (),
        max_output_tokens: int | None = None,
    ) -> routing.GenerationPlan:
        saved = await routing.saved_generation(self, ctx, role, invocation_id)
        if saved is not None and saved[0].contract_purpose is not None:
            definition = self.roles.resolve(saved[0].contract_purpose, saved[0].variant)
            prepared = None
        else:
            prepared = await self.prepare_role(
                ctx,
                role,
                variables,
                require_configured=False,
                context_messages=context_messages,
                max_output_tokens=max_output_tokens,
            )
            definition = prepared.definition
        plan = await routing.generation_plan(
            self,
            ctx,
            role,
            variables,
            definition.output_type,
            source_ids,
            media,
            invocation_id,
            "invocation",
            prepared=prepared,
            context_messages=context_messages,
            max_output_tokens=max_output_tokens,
        )
        return plan

    generate = routing.generate
    embed = routing.embed
    embed_media = routing.embed_media
    reusable_embedding_in = reusable_embedding_in
    find_embedding_in = find_embedding_in
    model_contract = model_contract
    model_preview = model_preview
    prepare_role = prepare_role

    def __init__(
        self,
        authority: Authority,
        settings: Settings,
        prompts: PromptService,
        configuration: ConfigurationService,
        network: NetworkService,
        *,
        roles: RoleRegistry,
        media_resolver: MediaResolver | None = None,
        preview_media_resolver: Callable[[CallContext, MediaReference], Awaitable[MediaInput]]
        | None = None,
    ) -> None:
        self.settings = settings
        self.prompts = prompts
        self.configuration = configuration
        self.ledger = CallLedger(authority)
        self.network = network
        self.generation = Generation(self.ledger, network)
        self.media_resolver = media_resolver
        self.preview_media_resolver = preview_media_resolver
        self.roles = roles

    async def status(self, ctx: CallContext) -> dict[str, object]:
        """Report configured resources and local credential readiness without probing a provider."""
        effective = await self.configuration.effective(ctx, "models")
        resources = ModelConfiguration.model_validate(effective["value"])
        roles: dict[str, object] = {}
        for role_id, role in resources.roles.items():
            routes: list[dict[str, object]] = []
            for index, route_id in enumerate(role.preset_ids()):
                model = resources.catalog[resources.presets[route_id].model]
                ready = False
                failure: dict[str, object] | None = None
                try:
                    resolved, _ = resources.resolve(role_id, index)
                    await asyncio.to_thread(credential, resolved, role_id)
                except DomainError as error:
                    if error.code != "model_unavailable":
                        raise
                    failure = {"code": error.code, "message": str(error), "context": error.details}
                else:
                    ready = True
                routes.append(
                    {
                        "id": route_id,
                        "model": model.actual_name,
                        "provider": model.provider,
                        "capabilities": list(model.input_modalities),
                        "dimensions": model.dimensions,
                        "normalization": model.normalization,
                        "max_batch_items": model.max_batch_items
                        if role.purpose == "embedding"
                        else None,
                        "video_content_format": model.video_content_format,
                        "credential_ready": ready,
                        "unavailable": failure,
                        "model_configured": bool(model.actual_name),
                    }
                )
            roles[role_id] = {
                "purpose": role.purpose,
                "prompt_id": role.prompt_id,
                "active_route": role.preset_ids()[0],
                "routes": routes,
                "max_calls_per_day": role.budget.max_calls_per_day,
            }
        return {"origin": effective["origin"], "roles": roles, "provider_probed": False}

    async def render_prompt(
        self, ctx: CallContext, role_id: str, variables: dict[str, object], prompt_id: str
    ) -> CompiledPrompt:
        try:
            return await self.prompts.render(ctx, variables, prompt_id)
        except KeyError as error:
            raise DomainError(
                "model_unavailable",
                "Model prompt resource is not configured",
                role=role_id,
                prompt_id=prompt_id,
                reason="prompt_missing",
            ) from error

    async def prepare_embedding(self, ctx: CallContext) -> EmbeddingProfile:
        resources = ModelConfiguration.model_validate(
            (await self.configuration.effective(ctx, "models"))["value"]
        )
        role = resources.roles.get("embedding")
        if role is None or role.purpose != "embedding":
            raise DomainError(
                "model_unavailable",
                "The embedding role is not configured for embedding",
                role="embedding",
            )
        routes: list[EmbeddingRoute] = []
        for index in range(len(role.preset_ids())):
            route, _ = resources.resolve("embedding", index, require_configured=False)
            if not isinstance(route, EmbeddingRoute):
                raise DomainError(
                    "model_unavailable",
                    "Embedding role requires an embedding route",
                    role="embedding",
                )
            routes.append(route)
        return EmbeddingProfile(
            route=routes[0],
            fallbacks=tuple(routes[1:]),
            route_ids=role.preset_ids(),
            fallback_policy=role.failure_policy,
        )

    async def successful_embedding_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        call_id: str,
    ) -> EmbeddingBatch:
        paid = await self.ledger.successful_in(view, ctx, call_id)
        batch = retained_embedding(paid.result, paid.origin_call_id or paid.id)
        return batch.model_copy(update={"call_ids": (paid.id,) * len(batch.vectors)})
