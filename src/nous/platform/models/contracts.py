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
    route: ModelRoute
    role: RoleResource
    definition: RoleDefinition
    prompt: dict[str, object]
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
    route, role = resources.resolve(role_id, require_configured=require_configured)
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
    model = resources.models[resources.routes[role.routes[0]].model]
    return PreparedRole(route, role, definition, prompt, model)


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
    invocation = prepare_invocation(
        prepared.definition.invocation_purpose or request.role,
        prepared.route,
        prepared.prompt,
        prepared.definition.output_type,
        tuple(media),
        request.source_ids,
    )
    definition = prepared.definition
    contract = ModelRoleContract(
        role=request.role,
        purpose=prepared.role.purpose,
        variant=definition.variant,
        variants=self.roles.variants(prepared.role.purpose),
        input_schema=json_object(definition.input_type.model_json_schema()),
        output_schema=json_object(TypeAdapter(definition.output_type).json_schema()),
        output_mode="text" if definition.output_type is str else "json_schema",
        provider_response_formats=response_formats(definition.output_type, prepared.route.model),
        prompt_id=prepared.role.prompt_id,
        prompt=ModelPrompt.model_validate(prepared.prompt),
    )
    return ModelInvocationPreview(
        contract=contract,
        invocation=InvocationSnapshot.model_validate(invocation),
        capabilities=prepared.model.capabilities,
        configured=bool(prepared.route.model and prepared.route.base_url),
    )
