"""Provider connections, factual model capabilities, presets, and task roles."""

from pathlib import Path
from typing import Literal

from nous_sdk.contracts.common import DomainError
from nous_sdk.contracts.model_routing import ModelRoutingPolicy
from pydantic import BaseModel, ConfigDict, Field, model_validator

from .settings import EmbeddingRoute, ModelRoute, ProviderOptions


class Resource(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class ProviderResource(Resource):
    protocol: Literal["openai-chat", "openai-responses"] = "openai-chat"
    base_url: str = "https://api.openai.com/v1"
    credential_file: Path | None = None
    credential_env: str = "NOUS_MODEL_API_KEY"
    timeout_seconds: float = Field(default=120, gt=0)


class ModelResource(Resource):
    max_requests_per_day: int = Field(default=50, ge=0)
    provider: str
    actual_name: str = ""
    tokenizer: str | None = None
    capability_source: str | None = None
    operations: tuple[Literal["generation", "embedding", "rerank", "transcription"], ...]
    input_modalities: tuple[Literal["text", "image", "audio", "video", "document"], ...] = ("text",)
    output_modalities: tuple[Literal["text", "image", "audio", "video", "document"], ...] = (
        "text",
    )
    context_window: int = Field(default=32768, gt=0)
    max_output_tokens: int = Field(default=8192, gt=0)
    structured_output_mode: Literal["native", "tool", "text"] = "native"
    tools: bool = False
    cache_capabilities: tuple[str, ...] = ()
    allowed_provider_options: tuple[str, ...] = ()
    allowed_reasoning_options: tuple[str, ...] = ("effort",)
    media_limits: dict[str, int] = Field(default_factory=dict)
    dimensions: int | None = Field(default=None, gt=0)
    normalization: Literal["none", "l2"] = "none"
    media_path: str = "embeddings/multimodal"
    max_batch_items: int = Field(default=64, ge=1, le=2048)
    video_content_format: Literal["video_url"] | None = None


class PresetResource(Resource):
    model: str
    max_output_tokens: int = Field(default=4096, gt=0)
    temperature: float | None = Field(default=None, ge=0, le=2)
    reasoning_options: dict[str, str | int | bool] = Field(default_factory=dict)
    timeout_seconds: float | None = Field(default=None, gt=0)
    provider_options: ProviderOptions = Field(default_factory=dict)


class RoleBudget(Resource):
    max_calls_per_day: int = Field(default=50, ge=0)


class RoleResource(Resource):
    failure_policy: ModelRoutingPolicy = Field(default_factory=ModelRoutingPolicy)
    preset: str
    allowed_fallback_presets: tuple[str, ...] = ()
    purpose: str = Field(min_length=1)
    operation: Literal["generation", "embedding", "rerank", "transcription"] = "generation"
    output_mode: Literal["text", "json_schema"] = "json_schema"
    prompt_id: str = "formation"
    budget: RoleBudget = Field(default_factory=RoleBudget)
    automatic_enabled: bool = False

    def preset_ids(self) -> tuple[str, ...]:
        return (self.preset, *self.allowed_fallback_presets)


class ModelConfiguration(Resource):
    providers: dict[str, ProviderResource]
    catalog: dict[str, ModelResource]
    presets: dict[str, PresetResource]
    roles: dict[str, RoleResource]

    @model_validator(mode="after")
    def references_exist(self) -> ModelConfiguration:
        for model_id, model in self.catalog.items():
            if model.provider not in self.providers:
                raise ValueError(
                    f"models.catalog.{model_id}.provider: unknown provider {model.provider}"
                )
            if "embedding" in model.operations and model.dimensions is None:
                raise ValueError(
                    f"models.catalog.{model_id}.dimensions: embedding dimension is required"
                )
        for preset_id, preset in self.presets.items():
            if preset.model not in self.catalog:
                raise ValueError(f"models.presets.{preset_id}.model: unknown model {preset.model}")
            model = self.catalog[preset.model]
            if preset.max_output_tokens > min(model.max_output_tokens, model.context_window):
                raise ValueError(
                    f"models.presets.{preset_id}.max_output_tokens exceeds model {preset.model}"
                )
            unsupported = set(preset.provider_options).difference(model.allowed_provider_options)
            if unsupported:
                raise ValueError(
                    f"models.presets.{preset_id}.provider_options: model {preset.model} does not support {sorted(unsupported)}"
                )
            unsupported = set(preset.reasoning_options).difference(model.allowed_reasoning_options)
            if unsupported:
                raise ValueError(
                    f"models.presets.{preset_id}.reasoning_options: model {preset.model} does not support {sorted(unsupported)}"
                )
        for role_id, role in self.roles.items():
            for preset_id in role.preset_ids():
                if preset_id not in self.presets:
                    raise ValueError(f"models.roles.{role_id}: unknown preset {preset_id}")
                model = self.catalog[self.presets[preset_id].model]
                if (
                    role.output_mode == "json_schema"
                    and role.operation == "generation"
                    and model.structured_output_mode == "text"
                ):
                    raise ValueError(
                        f"models.roles.{role_id}: model {model.actual_name} requires native structured output or tools"
                    )
                if role.operation not in model.operations:
                    raise ValueError(
                        f"models.roles.{role_id}: model {model.actual_name} does not support {role.operation}"
                    )
        return self

    def resolve(
        self, role_id: str, route_index: int = 0, *, require_configured: bool = True
    ) -> tuple[ModelRoute | EmbeddingRoute, RoleResource]:
        try:
            role = self.roles[role_id]
            preset = self.presets[role.preset_ids()[route_index]]
            model = self.catalog[preset.model]
            provider = self.providers[model.provider]
        except (KeyError, IndexError) as error:
            raise DomainError(
                "model_unavailable",
                "Model role or preset is not configured",
                role=role_id,
                reason="resource_missing",
            ) from error
        if require_configured and (not model.actual_name.strip() or not provider.base_url.strip()):
            raise DomainError(
                "model_unavailable",
                "Model or provider connection is not configured",
                role=role_id,
                reason="model_missing",
            )
        values = {
            **provider.model_dump(exclude={"timeout_seconds"}),
            "model": model.actual_name,
            "timeout_seconds": preset.timeout_seconds or provider.timeout_seconds,
            "max_output_tokens": preset.max_output_tokens,
            "max_calls_per_day": role.budget.max_calls_per_day,
            "resource_request_budget": model.max_requests_per_day,
            "temperature": preset.temperature,
            "reasoning_effort": preset.reasoning_options.get("effort"),
            "extra_body": preset.provider_options,
            "video_content_format": model.video_content_format,
            "input_modalities": model.input_modalities,
            "context_window": model.context_window,
            "structured_output_mode": model.structured_output_mode,
            "tools": model.tools,
            "media_limits": model.media_limits,
            "tokenizer": model.tokenizer,
        }
        if role.operation == "embedding":
            return EmbeddingRoute.model_validate(
                {
                    **values,
                    "dimensions": model.dimensions,
                    "normalization": model.normalization,
                    "media_path": model.media_path,
                    "max_batch_items": model.max_batch_items,
                }
            ), role
        return ModelRoute.model_validate(values), role


def initial_resources() -> ModelConfiguration:
    generation, embedding = ModelRoute(), EmbeddingRoute()
    generation_provider = ProviderResource()
    embedding_provider = ProviderResource(credential_env="NOUS_EMBEDDING_API_KEY")
    return ModelConfiguration(
        providers={"generation": generation_provider, "embedding": embedding_provider},
        catalog={
            "generation": ModelResource(
                provider="generation",
                actual_name=generation.model,
                operations=("generation",),
                input_modalities=("text",),
                video_content_format=generation.video_content_format,
            ),
            "embedding": ModelResource(
                provider="embedding",
                actual_name=embedding.model,
                operations=("embedding",),
                dimensions=embedding.dimensions,
                normalization=embedding.normalization,
                media_path=embedding.media_path,
                max_batch_items=embedding.max_batch_items,
            ),
        },
        presets={
            "formation": PresetResource(
                model="generation",
                max_output_tokens=generation.max_output_tokens,
                temperature=generation.temperature,
                reasoning_options={},
                provider_options=generation.extra_body,
            ),
            "embedding": PresetResource(model="embedding", provider_options=embedding.extra_body),
        },
        roles={
            "formation": RoleResource(
                preset="formation",
                purpose="formation",
                budget=RoleBudget(max_calls_per_day=generation.max_calls_per_day),
            ),
            "description": RoleResource(
                preset="formation",
                purpose="description",
                output_mode="text",
                prompt_id="media.describe",
                budget=RoleBudget(max_calls_per_day=generation.max_calls_per_day),
            ),
            "embedding": RoleResource(
                preset="embedding",
                purpose="embedding",
                operation="embedding",
                budget=RoleBudget(max_calls_per_day=embedding.max_calls_per_day),
            ),
            "steward": RoleResource(
                preset="formation",
                purpose="steward",
                prompt_id="context-steward",
                budget=RoleBudget(max_calls_per_day=generation.max_calls_per_day),
            ),
            "seed": RoleResource(
                preset="formation",
                purpose="seed",
                prompt_id="seed",
                budget=RoleBudget(max_calls_per_day=generation.max_calls_per_day),
            ),
            "correction": RoleResource(
                preset="formation",
                purpose="correction",
                prompt_id="correction",
                budget=RoleBudget(max_calls_per_day=generation.max_calls_per_day),
            ),
            "feedback": RoleResource(
                preset="formation",
                purpose="feedback",
                prompt_id="feedback",
                budget=RoleBudget(max_calls_per_day=generation.max_calls_per_day),
            ),
            "motivation": RoleResource(
                preset="formation",
                purpose="motivation",
                prompt_id="motivation",
                budget=RoleBudget(max_calls_per_day=generation.max_calls_per_day),
            ),
            "symbols": RoleResource(
                preset="formation",
                purpose="symbols",
                prompt_id="social-symbols",
                budget=RoleBudget(max_calls_per_day=generation.max_calls_per_day),
            ),
            "person_profile": RoleResource(
                preset="formation",
                purpose="person_profile",
                prompt_id="person-profile",
                budget=RoleBudget(max_calls_per_day=generation.max_calls_per_day),
            ),
            "episode": RoleResource(
                preset="formation",
                purpose="episode",
                prompt_id="episode-segmentation",
                budget=RoleBudget(max_calls_per_day=generation.max_calls_per_day),
            ),
            "journal": RoleResource(preset="formation", purpose="journal", prompt_id="journal"),
            "reflect": RoleResource(preset="formation", purpose="reflect", prompt_id="reflect"),
            "dream": RoleResource(preset="formation", purpose="dream", prompt_id="dream"),
            "context_compress": RoleResource(
                preset="formation",
                purpose="context_compress",
                prompt_id="context-compress",
                output_mode="text",
            ),
            "work_summary": RoleResource(
                preset="formation",
                purpose="work_summary",
                prompt_id="work-summary",
                output_mode="text",
            ),
            "work_questions": RoleResource(
                preset="formation", purpose="work_questions", prompt_id="work-questions"
            ),
            "query_rerank": RoleResource(
                preset="formation",
                purpose="rerank",
                prompt_id="query-rerank",
                budget=RoleBudget(max_calls_per_day=generation.max_calls_per_day),
            ),
        },
    )
