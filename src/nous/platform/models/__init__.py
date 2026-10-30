"""Native provider SDKs share managed clients, prompts and the paid call ledger."""

import asyncio
import os
from collections.abc import Awaitable, Callable

from nous_wave.contracts import (
    CallContext,
    DomainError,
    EmbeddingBatch,
    MediaReference,
)

from nous.platform.configuration import EmbeddingRoute, ModelRoute, Settings
from nous.platform.configuration.models import ModelConfiguration
from nous.platform.configuration.service import ConfigurationService
from nous.platform.network.service import NetworkService
from nous.platform.persistence import Authority, ReadView, WriteTx
from nous.platform.prompts import PromptService

from . import routing
from .contracts import model_contract, model_preview, prepare_role
from .embedding import MediaResolver, retained_embedding
from .embedding_evidence import reusable_embedding_in
from .embedding_profile import EmbeddingProfile
from .generation import (
    Generation,
    MediaInput,
)
from .ledger import CallLedger
from .roles import RoleRegistry

os.environ.setdefault("PYDANTIC_AI_NO_BANNER", "1")


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
    generate = routing.generate
    embed = routing.embed
    embed_media = routing.embed_media
    reusable_embedding_in = reusable_embedding_in
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
    ) -> None:
        self.settings = settings
        self.prompts = prompts
        self.configuration = configuration
        self.ledger = CallLedger(authority)
        self.network = network
        self.generation = Generation(self.ledger, network)
        self.media_resolver: MediaResolver | None = None
        self.preview_media_resolver: (
            Callable[[CallContext, MediaReference], Awaitable[MediaInput]] | None
        ) = None
        self.roles = roles

    async def status(self, ctx: CallContext) -> dict[str, object]:
        """Report configured resources and local credential readiness without probing a provider."""
        effective = await self.configuration.effective(ctx, "models")
        resources = ModelConfiguration.model_validate(effective["value"])
        roles: dict[str, object] = {}
        for role_id, role in resources.roles.items():
            routes: list[dict[str, object]] = []
            for index, route_id in enumerate(role.routes):
                model = resources.models[resources.routes[route_id].model]
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
                        "model": model.name,
                        "provider": model.provider,
                        "capabilities": model.capabilities,
                        "dimensions": model.dimensions,
                        "normalization": model.normalization,
                        "max_batch_items": model.max_batch_items
                        if role.purpose == "embedding"
                        else None,
                        "video_content_format": model.video_content_format,
                        "credential_ready": ready,
                        "unavailable": failure,
                        "model_configured": bool(model.name),
                    }
                )
            roles[role_id] = {
                "purpose": role.purpose,
                "prompt_id": role.prompt_id,
                "active_route": role.routes[0],
                "routes": routes,
                "max_calls_per_day": role.max_calls_per_day,
            }
        return {"origin": effective["origin"], "roles": roles, "provider_probed": False}

    async def render_prompt(
        self, ctx: CallContext, role_id: str, variables: dict[str, object], prompt_id: str
    ) -> dict[str, object]:
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
        for index in range(len(role.routes)):
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
            route_ids=role.routes,
            fallback_policy=role.fallback,
        )

    async def successful_embedding_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        call_id: str,
    ) -> EmbeddingBatch:
        paid = await self.ledger.successful_in(view, ctx, call_id)
        return retained_embedding(paid.result, paid.id)
