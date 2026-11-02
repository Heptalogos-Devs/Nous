"""Role contracts and previews built from prepared provider requests."""

from __future__ import annotations

import math
from dataclasses import dataclass
from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.model_roles import (
    ModelContractRequest,
    ModelInvocationPreview,
    ModelPreviewRequest,
    ModelPrompt,
    ModelRoleContract,
)
from nous_sdk.contracts.prompt_compilation import CompiledPrompt, PromptMessage, TokenEstimate
from pydantic import TypeAdapter
from typing_extensions import TypeForm

from nous.platform.configuration import ModelRoute
from nous.platform.configuration.models import ModelConfiguration, ModelResource, RoleResource

from .generation import MediaInput
from .provider_schema import response_format
from .roles import RoleDefinition, json_object

if TYPE_CHECKING:
    from . import ModelService


@dataclass(frozen=True)
class PreparedRole:
    routes: tuple[PreparedRoute, ...]
    role: RoleResource
    definition: RoleDefinition
    prompt: CompiledPrompt


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
    output_type: TypeForm[object] | None = None,
    variant: str | None = None,
    require_configured: bool = True,
    draft_template: str | None = None,
    context_messages: tuple[PromptMessage, ...] = (),
    max_output_tokens: int | None = None,
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
            resources.resolve(role_id, index, require_configured=require_configured)[0].model_copy(
                update={
                    "max_output_tokens": min(
                        max_output_tokens, resources.presets[route_id].max_output_tokens
                    )
                }
            )
            if max_output_tokens is not None
            else resources.resolve(role_id, index, require_configured=require_configured)[0],
            resources.catalog[resources.presets[route_id].model],
        )
        for index, route_id in enumerate(role.preset_ids())
    )
    if max_output_tokens is not None and max_output_tokens < 1:
        raise DomainError("model_output_budget", "The task output allocation must be positive")
    definition = self.roles.resolve(role.purpose, variant, output_type, role.output_mode)
    parsed, variables = definition.variables(variables)
    prompt = (
        await self.render_prompt(ctx, role_id, variables, definition.template_id)
        if draft_template is None
        else self.prompts.preview(
            {
                "id": definition.template_id,
                "revision": None,
                "template": draft_template,
                "active": False,
            },
            variables,
        )
    )
    messages = (
        tuple(message for message in prompt.messages if message.role == "system")
        + context_messages
        + tuple(message for message in prompt.messages if message.role != "system")
    )
    prompt = prompt.model_copy(
        update={
            "messages": messages,
            "token_estimate": TokenEstimate(
                text_tokens=math.ceil(
                    sum(len(message.content.encode("utf-8")) for message in messages) / 3
                ),
                method="utf8_bytes_div_3",
            ),
        }
    )
    bindings = {}
    source = variables.get("source")
    if isinstance(source, dict):
        bindings["source"] = {
            key: value
            for key, value in source.items()
            if key in {"material_id", "observation_id", "source_material_ids"}
        }
    prompt = prompt.model_copy(
        update={
            "input_bindings": json_object(bindings),
            "output_schema": None
            if definition.output_type is str
            else json_object(TypeAdapter(definition.output_type).json_schema()),
        }
    )
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
        prompt = await self.prompts.get(ctx, definition.template_id)
    except KeyError as error:
        raise DomainError(
            "model_unavailable",
            "Model prompt resource is not configured",
            role=request.role,
            template_id=definition.template_id,
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
        provider_response_format=response_format(
            definition.output_type,
            resources.resolve(request.role, require_configured=False)[0].model,
            mode=role.output_mode,
        ),
        template_id=definition.template_id,
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
        context_messages=request.context_messages,
        max_output_tokens=request.max_output_tokens,
    )
    media: list[MediaInput] = []
    if request.media and self.preview_media_resolver is None:
        raise DomainError("model_contract_unavailable", "Media input owner is unavailable")
    for reference in request.media:
        if self.preview_media_resolver is not None:
            media.append(await self.preview_media_resolver(ctx, reference))
    definition = prepared.definition
    selected = prepared.routes[0]
    prepared_request = self.generation.prepare(
        request.role,
        selected.route,
        prepared.prompt,
        definition.output_type,
        tuple(media),
        request.source_ids,
        "none",
    )
    contract = ModelRoleContract(
        role=request.role,
        purpose=prepared.role.purpose,
        variant=definition.variant,
        variants=self.roles.variants(prepared.role.purpose),
        input_schema=json_object(definition.input_type.model_json_schema()),
        output_schema=json_object(TypeAdapter(definition.output_type).json_schema()),
        output_mode="text" if definition.output_type is str else "json_schema",
        provider_response_format=response_format(
            definition.output_type, selected.route.model, mode=prepared.role.output_mode
        ),
        template_id=definition.template_id,
        prompt=ModelPrompt.model_validate(await self.prompts.get(ctx, definition.template_id)),
    )
    return ModelInvocationPreview(
        contract=contract,
        invocation=prepared_request.preview,
        capabilities=selected.model.input_modalities,
        configured=bool(selected.route.model and selected.route.base_url),
    )
