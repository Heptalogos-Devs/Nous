"""Pure provider-neutral resource definitions. Loading never contacts providers."""

from typing import Annotated, Literal

from pydantic import Field, JsonValue

from nous.configuration.metadata import ConfigModel, setting
from nous.runtime.values import ResourceId

Capability = Literal[
    "text", "image", "audio", "video", "stream", "structured_output", "tools", "embedding"
]


class EnvironmentCredential(ConfigModel):
    kind: Literal["env"]
    name: str = Field(min_length=1)


class FileCredential(ConfigModel):
    kind: Literal["file"]
    path: str = Field(min_length=1)


CredentialReference = Annotated[EnvironmentCredential | FileCredential, Field(discriminator="kind")]


class ProviderConfig(ConfigModel):
    protocol: str = Field(min_length=1)
    endpoint: str = Field(pattern=r"^https?://[^\s]+$")
    credential_ref: ResourceId | None = None
    network: NetworkOverride = Field(default_factory=lambda: NetworkOverride())


class NetworkOverride(ConfigModel):
    proxy_url: str | None = None
    trust_env: bool | None = None
    ca_file: str | None = None
    connect_timeout_seconds: float | None = Field(default=None, gt=0)
    read_timeout_seconds: float | None = Field(default=None, gt=0)
    write_timeout_seconds: float | None = Field(default=None, gt=0)
    pool_timeout_seconds: float | None = Field(default=None, gt=0)
    max_connections: int | None = Field(default=None, ge=1)


class ModelConfig(ConfigModel):
    provider: ResourceId
    model_name: str = Field(min_length=1)
    capabilities: list[Capability] = Field(default_factory=list)
    context_window: int | None = Field(default=None, ge=1)
    max_output_tokens: int | None = Field(default=None, ge=1)
    deployment_revision: str | None = None


class PresetConfig(ConfigModel):
    temperature: float | None = Field(default=None, ge=0)
    top_p: float | None = Field(default=None, ge=0, le=1)
    reasoning: dict[str, JsonValue] = Field(default_factory=dict)
    max_output_tokens: int | None = Field(default=None, ge=1)
    timeout_seconds: float | None = Field(default=None, gt=0)
    extensions: dict[str, JsonValue] = Field(default_factory=dict)


class Budget(ConfigModel):
    max_attempts: int | None = Field(default=None, ge=1)
    timeout_seconds: float | None = Field(default=None, gt=0)
    max_input_tokens: int | None = Field(default=None, ge=1)
    max_output_tokens: int | None = Field(default=None, ge=1)
    max_cost: float | None = Field(default=None, ge=0)


class Fallback(ConfigModel):
    model: ResourceId
    preset: ResourceId | None = None


class TaskAssignment(ConfigModel):
    model: ResourceId
    prompt: str = Field(min_length=1)
    preset: ResourceId | None = None
    fallback: list[Fallback] = Field(default_factory=list)
    budget: Budget = Field(default_factory=Budget)
    media_strategy: (
        Literal["description_only", "direct_structured", "describe_then_structure"] | None
    ) = None


class ModelExecutionConfig(ConfigModel):
    max_attempts: int = setting(
        2,
        ge=1,
        description="Maximum attempts; narrower scopes may lower this limit.",
        scopes=("instance", "namespace", "task", "call"),
    )
    timeout_seconds: float = setting(
        120,
        gt=0,
        description="Model operation deadline in seconds.",
        scopes=("instance", "namespace", "task", "call"),
    )
    max_input_tokens: int = setting(
        16384,
        ge=1,
        description="Instance input token ceiling.",
        scopes=("instance", "namespace", "task", "call"),
    )
    max_output_tokens: int = setting(
        2048,
        ge=1,
        description="Instance output token ceiling.",
        scopes=("instance", "namespace", "task", "call"),
    )
    structured_retries: int = setting(
        0,
        ge=0,
        description="Retries for structured output.",
        scopes=("instance", "namespace", "task", "call"),
    )


class PromptConfig(ConfigModel):
    cache_entries: int = setting(
        128, ge=0, apply="restart", description="Compiled prompt cache capacity."
    )
    bytecode_cache: bool = setting(
        True, apply="restart", description="Enable prompt bytecode cache."
    )


class EncodingConfig(ConfigModel):
    document_instruction: str
    query_instruction: str
    preprocessing_revision: str = Field(min_length=1)
    input_combination: Literal["text", "ordered_multimodal"]


class EmbeddingSpace(ConfigModel):
    model: ResourceId
    deployment_revision: str = Field(min_length=1)
    dimensions: int = Field(gt=0)
    modalities: list[Literal["text", "image", "audio", "video"]] = Field(min_length=1)
    normalization: Literal["none", "l2"]
    encoding: EncodingConfig


class EmbeddingConfig(ConfigModel):
    spaces: dict[ResourceId, EmbeddingSpace] = setting(
        {},
        apply="new_binding",
        impact="data_bound",
        level="standard",
        description="A space fixes model deployment, dimensions and input encoding; used spaces require a new ID when these change.",
    )
