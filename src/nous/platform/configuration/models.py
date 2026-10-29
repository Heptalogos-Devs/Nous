"""Providers, model capabilities, routes and roles have distinct identities."""

from pathlib import Path
from typing import Literal

from nous_wave.contracts.common import DomainError
from pydantic import BaseModel, Field, model_validator

from .settings import EmbeddingRoute, ModelRoute, ProviderOptions, ReasoningEffort


class ProviderResource(BaseModel):
    base_url: str
    credential_file: Path | None = None
    credential_env: str = "NOUS_MODEL_API_KEY"
    timeout_seconds: float = Field(default=120, gt=0)


class ModelResource(BaseModel):
    provider: str
    name: str
    capabilities: tuple[Literal["text", "image", "audio", "video", "embedding"], ...]
    dimensions: int | None = Field(default=None, gt=0)
    normalization: Literal["none", "l2"] = "none"
    media_path: str = "embeddings/multimodal"
    max_batch_items: int = Field(default=64, ge=1, le=2048)
    video_content_format: Literal["video_url"] | None = None


class RouteResource(BaseModel):
    model: str
    max_output_tokens: int = Field(default=8192, gt=0)
    temperature: float | None = Field(default=None, ge=0, le=2)
    reasoning_effort: ReasoningEffort | None = None
    extra_body: ProviderOptions = Field(default_factory=dict)


class RoleResource(BaseModel):
    routes: tuple[str, ...] = Field(min_length=1)
    purpose: Literal["formation", "description", "embedding", "steward", "maintenance", "rerank"]
    prompt_id: str = "formation"
    max_calls_per_day: int = Field(default=50, ge=0)


class ModelConfiguration(BaseModel):
    providers: dict[str, ProviderResource]
    models: dict[str, ModelResource]
    routes: dict[str, RouteResource]
    roles: dict[str, RoleResource]

    @model_validator(mode="after")
    def references_exist(self) -> ModelConfiguration:
        for model in self.models.values():
            if model.provider not in self.providers:
                raise ValueError(f"Unknown provider {model.provider}")
            if "embedding" in model.capabilities and model.dimensions is None:
                raise ValueError("Embedding models require an explicit vector dimension")
        for route in self.routes.values():
            if route.model not in self.models:
                raise ValueError(f"Unknown model resource {route.model}")
        for role in self.roles.values():
            for route_id in role.routes:
                if route_id not in self.routes:
                    raise ValueError(f"Unknown route {route_id}")
                model = self.models[self.routes[route_id].model]
                capability = "embedding" if role.purpose == "embedding" else "text"
                if capability not in model.capabilities:
                    raise ValueError(f"Role {role.purpose} requires {capability}")
        return self

    def resolve(
        self, role_id: str, route_index: int = 0, *, require_configured: bool = True
    ) -> tuple[ModelRoute | EmbeddingRoute, RoleResource]:
        try:
            role = self.roles[role_id]
            route = self.routes[role.routes[route_index]]
            model = self.models[route.model]
            provider = self.providers[model.provider]
        except (KeyError, IndexError) as error:
            raise DomainError(
                "model_unavailable",
                "Model role or route is not configured",
                role=role_id,
                reason="resource_missing",
            ) from error
        if require_configured and (not model.name.strip() or not provider.base_url.strip()):
            raise DomainError(
                "model_unavailable",
                "Model or provider connection is not configured",
                role=role_id,
                reason="model_missing",
            )
        values = {
            **provider.model_dump(),
            "model": model.name,
            "max_output_tokens": route.max_output_tokens,
            "max_calls_per_day": role.max_calls_per_day,
            "temperature": route.temperature,
            "reasoning_effort": route.reasoning_effort,
            "extra_body": route.extra_body,
            "video_content_format": model.video_content_format,
        }
        if role.purpose == "embedding":
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


def initial_resources(generation: ModelRoute, embedding: EmbeddingRoute) -> ModelConfiguration:
    generation_provider = ProviderResource.model_validate(generation.model_dump(), extra="ignore")
    embedding_provider = ProviderResource.model_validate(embedding.model_dump(), extra="ignore")
    return ModelConfiguration(
        providers={"generation": generation_provider, "embedding": embedding_provider},
        models={
            "generation": ModelResource(
                provider="generation",
                name=generation.model,
                capabilities=("text", "image", "audio", "video"),
                video_content_format=generation.video_content_format,
            ),
            "embedding": ModelResource(
                provider="embedding",
                name=embedding.model,
                capabilities=("embedding",),
                dimensions=embedding.dimensions,
                normalization=embedding.normalization,
                media_path=embedding.media_path,
                max_batch_items=embedding.max_batch_items,
            ),
        },
        routes={
            "formation": RouteResource(
                model="generation",
                max_output_tokens=generation.max_output_tokens,
                temperature=generation.temperature,
                reasoning_effort=generation.reasoning_effort,
                extra_body=generation.extra_body,
            ),
            "embedding": RouteResource(model="embedding", extra_body=embedding.extra_body),
        },
        roles={
            "formation": RoleResource(
                routes=("formation",),
                purpose="formation",
                max_calls_per_day=generation.max_calls_per_day,
            ),
            "description": RoleResource(
                routes=("formation",),
                purpose="description",
                prompt_id="media.describe",
                max_calls_per_day=generation.max_calls_per_day,
            ),
            "embedding": RoleResource(
                routes=("embedding",),
                purpose="embedding",
                max_calls_per_day=embedding.max_calls_per_day,
            ),
            "steward": RoleResource(
                routes=("formation",),
                purpose="steward",
                prompt_id="context-steward",
                max_calls_per_day=generation.max_calls_per_day,
            ),
            "maintenance": RoleResource(
                routes=("formation",),
                purpose="maintenance",
                prompt_id="maintenance",
                max_calls_per_day=generation.max_calls_per_day,
            ),
        },
    )
