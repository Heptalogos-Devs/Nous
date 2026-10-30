"""Read-only contracts and actual preparation shared with paid execution."""

from __future__ import annotations

import json
from dataclasses import dataclass
from hashlib import sha256
from typing import TYPE_CHECKING

from nous_wave.contracts import CallContext, DomainError
from nous_wave.contracts.model_roles import (
    InvocationSnapshot,
    ModelContractRequest,
    ModelInvocationPreview,
    ModelPreviewRequest,
    ModelPrompt,
    ModelRoleContract,
    ModelRoutePreview,
)
from pydantic import BaseModel, TypeAdapter

from nous.platform.configuration import ModelRoute
from nous.platform.configuration.models import ModelConfiguration, ModelResource, RoleResource

from .generation import MediaInput, prepare_invocation
from .provider_schema import response_formats
from .roles import RoleDefinition, json_object

if TYPE_CHECKING:
    from . import ModelService


@dataclass(frozen=True)
class PreparedRole:
    routes: tuple[PreparedRoute, ...]
    role: RoleResource
    definition: RoleDefinition
    prompt: dict[str, object]


@dataclass(frozen=True)
class PreparedRoute:
    id: str
    index: int
    route: ModelRoute
    model: ModelResource


async def prepare_role(
    self: ModelService,
    ctx: CallContext,
    role_id: str,
    variables: dict[str, object],
    *,
    output_type: type[BaseModel] | type[str] | None = None,
    variant: str | None = None,
    require_configured: bool = True,
    draft_template: str | None = None,
) -> PreparedRole:
    resources = ModelConfiguration.model_validate(
        (await self.configuration.effective(ctx, "models"))["value"]
    )
    role = resources.roles.get(role_id)
    if role is None:
        raise DomainError("model_unavailable", "Role is not configured", role=role_id)
    routes = tuple(
        PreparedRoute(
            route_id,
            index,
            resources.resolve(role_id, index, require_configured=require_configured)[0],
            resources.models[resources.routes[route_id].model],
        )
        for index, route_id in enumerate(role.routes)
    )
    definition = self.roles.resolve(role.purpose, variant, output_type, role.output_mode)
    parsed, variables = definition.variables(variables)
    prompt = (
        await self.render_prompt(ctx, role_id, variables, role.prompt_id)
        if draft_template is None
        else self.prompts.preview(
            {"id": role.prompt_id, "revision": None, "template": draft_template, "active": False},
            variables,
        )
    )
    if not prompt["variable_names"]:
        prompt["rendered"] = (
            str(prompt["rendered"]) + "\nInput:\n" + json.dumps(variables, ensure_ascii=False)
        )
    if definition.transform_prompt:
        prompt = definition.transform_prompt(prompt, parsed)
    prompt["hash"] = sha256(str(prompt["rendered"]).encode()).hexdigest()
    return PreparedRole(routes, role, definition, prompt)


async def model_contract(
    self: ModelService, ctx: CallContext, request: ModelContractRequest
) -> ModelRoleContract:
    resources = ModelConfiguration.model_validate(
        (await self.configuration.effective(ctx, "models"))["value"]
    )
    try:
        role = resources.roles[request.role]
    except KeyError as error:
        raise DomainError(
            "model_contract_unavailable", "Role is not configured", role=request.role
        ) from error
    definition = self.roles.resolve(role.purpose, request.variant, output_mode=role.output_mode)
    try:
        prompt = await self.prompts.get(ctx, role.prompt_id)
    except KeyError as error:
        raise DomainError(
            "model_unavailable",
            "Model prompt resource is not configured",
            role=request.role,
            prompt_id=role.prompt_id,
            reason="prompt_missing",
        ) from error
    return ModelRoleContract(
        routing_policy=role.fallback,
        role=request.role,
        purpose=role.purpose,
        variant=definition.variant,
        variants=self.roles.variants(role.purpose),
        input_schema=json_object(definition.input_type.model_json_schema()),
        output_schema=json_object(TypeAdapter(definition.output_type).json_schema()),
        output_mode="text" if definition.output_type is str else "json_schema",
        provider_response_formats=response_formats(
            definition.output_type,
            resources.resolve(request.role, require_configured=False)[0].model,
        ),
        prompt_id=role.prompt_id,
        prompt=ModelPrompt.model_validate(prompt),
    )


async def model_preview(
    self: ModelService, ctx: CallContext, request: ModelPreviewRequest
) -> ModelInvocationPreview:
    prepared = await prepare_role(
        self,
        ctx,
        request.role,
        dict(request.input),
        variant=request.variant,
        require_configured=False,
        draft_template=request.draft_template,
    )
    media: list[MediaInput] = []
    if request.media and self.preview_media_resolver is None:
        raise DomainError("model_contract_unavailable", "Media input owner is unavailable")
    for reference in request.media:
        if self.preview_media_resolver is not None:
            media.append(await self.preview_media_resolver(ctx, reference))
    routes: list[ModelRoutePreview] = []
    for item in prepared.routes:
        try:
            invocation = prepare_invocation(
                prepared.definition.invocation_purpose or request.role,
                item.route,
                prepared.prompt,
                prepared.definition.output_type,
                tuple(media),
                request.source_ids,
            )
        except DomainError as error:
            if error.code != "model_unavailable":
                raise
            routes.append(
                ModelRoutePreview(
                    route_id=item.id,
                    route_index=item.index,
                    invocation=None,
                    capabilities=item.model.capabilities,
                    configured=False,
                    unavailable=json_object(
                        {"code": error.code, "message": str(error), "details": error.details}
                    ),
                )
            )
        else:
            routes.append(
                ModelRoutePreview(
                    route_id=item.id,
                    route_index=item.index,
                    invocation=InvocationSnapshot.model_validate(invocation),
                    capabilities=item.model.capabilities,
                    configured=bool(item.route.model and item.route.base_url),
                )
            )
    definition = prepared.definition
    contract = ModelRoleContract(
        routing_policy=prepared.role.fallback,
        role=request.role,
        purpose=prepared.role.purpose,
        variant=definition.variant,
        variants=self.roles.variants(prepared.role.purpose),
        input_schema=json_object(definition.input_type.model_json_schema()),
        output_schema=json_object(TypeAdapter(definition.output_type).json_schema()),
        output_mode="text" if definition.output_type is str else "json_schema",
        provider_response_formats=response_formats(
            definition.output_type, prepared.routes[0].route.model
        ),
        prompt_id=prepared.role.prompt_id,
        prompt=ModelPrompt.model_validate(prepared.prompt),
    )
    return ModelInvocationPreview(
        contract=contract,
        invocation=routes[0].invocation,
        routes=tuple(routes),
        capabilities=prepared.routes[0].model.capabilities,
        configured=routes[0].configured,
    )
