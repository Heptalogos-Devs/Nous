"""Pure provider-neutral resource definitions. Loading never contacts providers."""

from typing import Annotated, Literal

from pydantic import (
    Field,
    JsonValue,
    SerializerFunctionWrapHandler,
    model_serializer,
    model_validator,
)

from nous.common.values import ResourceId
from nous.configuration.metadata import ConfigJson, ConfigMap, ConfigModel, ConfigSequence, setting

Capability = Literal[
    "text", "image", "audio", "video", "stream", "structured_output", "tools", "embedding"
]


class EnvironmentCredential(ConfigModel):
    kind: Annotated[
        Literal["env"], setting(apply="restart", description="Environment credential source.")
    ]
    name: Annotated[
        str,
        setting(
            min_length=1,
            apply="restart",
            description="Environment variable containing the credential.",
        ),
    ]


class FileCredential(ConfigModel):
    kind: Annotated[
        Literal["file"], setting(apply="restart", description="File credential source.")
    ]
    path: Annotated[
        str,
        setting(
            min_length=1,
            apply="restart",
            description="Credential file, relative to the instance directory.",
        ),
    ]


CredentialReference = Annotated[EnvironmentCredential | FileCredential, Field(discriminator="kind")]


class NetworkOverride(ConfigModel):
    proxy_url: str | None = setting(
        None, apply="restart", description="Override the instance HTTP proxy."
    )
    trust_env: bool | None = setting(
        None, apply="restart", description="Override transport environment use."
    )
    ca_file: str | None = setting(
        None, apply="restart", description="Override the trusted CA bundle."
    )
    connect_timeout_seconds: float | None = setting(
        None, gt=0, description="Override connection timeout in seconds."
    )
    read_timeout_seconds: float | None = setting(
        None, gt=0, description="Override read timeout in seconds."
    )
    write_timeout_seconds: float | None = setting(
        None, gt=0, description="Override write timeout in seconds."
    )
    pool_timeout_seconds: float | None = setting(
        None, gt=0, description="Override connection-pool timeout in seconds."
    )
    max_connections: int | None = setting(
        None, ge=1, apply="restart", description="Override the connection pool ceiling."
    )

    @model_validator(mode="after")
    def explicit_null(self) -> NetworkOverride:
        for name in self.model_fields_set - {"proxy_url", "ca_file"}:
            if getattr(self, name) is None:
                raise ValueError(f"{name} cannot be null; omit it to inherit")
        return self

    @model_serializer(mode="wrap")
    def serialize_overrides(self, handler: SerializerFunctionWrapHandler) -> dict[str, JsonValue]:
        values = handler(self)
        return {name: values[name] for name in self.model_fields_set}


class ProviderConfig(ConfigModel):
    protocol: Annotated[
        str,
        setting(
            min_length=1, apply="restart", description="Explicit protocol used by this connection."
        ),
    ]
    endpoint: Annotated[
        str,
        setting(
            pattern=r"^https?://[^\s]+$", apply="restart", description="Protocol API base URL."
        ),
    ]
    credential_ref: ResourceId | None = setting(
        None, apply="restart", description="Configured credential reference."
    )
    network: NetworkOverride = Field(
        default_factory=NetworkOverride, description="Optional per-provider network overrides."
    )


class ModelConfig(ConfigModel):
    provider: ResourceId = Field(description="Configured provider connection ID.")
    model_name: str = Field(min_length=1, description="Model name sent to the configured gateway.")
    capabilities: ConfigSequence[Capability] = Field(
        default_factory=tuple, description="Explicitly confirmed model capabilities."
    )
    context_window: int | None = Field(
        default=None, ge=1, description="Optional model context limit in tokens."
    )
    max_output_tokens: int | None = Field(
        default=None, ge=1, description="Optional model output limit in tokens."
    )
    deployment_revision: str | None = Field(
        default=None, description="Operator-declared model deployment revision."
    )
    input_formats: ConfigSequence[str] = Field(
        default_factory=lambda: ("text/plain",), description="Accepted MIME formats."
    )
    output_modes: ConfigSequence[Literal["text", "native", "tool"]] = Field(
        default_factory=lambda: ("text",), description="Confirmed generation output modes."
    )
    compatibility: ConfigMap[str, ConfigJson] = Field(
        default_factory=dict, description="Explicit SDK protocol profile overrides."
    )


class PresetConfig(ConfigModel):
    temperature: float | None = Field(default=None, ge=0)
    top_p: float | None = Field(default=None, ge=0, le=1)
    reasoning: ConfigMap[str, ConfigJson] = Field(default_factory=dict)
    max_output_tokens: int | None = Field(default=None, ge=1)
    timeout_seconds: float | None = Field(default=None, gt=0)
    extensions: ConfigMap[str, ConfigJson] = Field(default_factory=dict)


class Budget(ConfigModel):
    max_attempts: int | None = Field(default=None, ge=1, description="Per-stage attempt limit.")
    timeout_seconds: float | None = Field(
        default=None, gt=0, description="Stage timeout in seconds."
    )
    max_input_tokens: int | None = Field(default=None, ge=1, description="Input token limit.")
    max_output_tokens: int | None = Field(default=None, ge=1, description="Output token limit.")
    max_cost: float | None = Field(
        default=None, ge=0, description="Monetary limits are disabled; this value must be null."
    )

    @model_validator(mode="after")
    def cost_limits_disabled(self) -> Budget:
        if self.max_cost is not None:
            raise ValueError("Monetary model limits are not enabled; max_cost must be null")
        return self


class Fallback(ConfigModel):
    model: ResourceId
    preset: ResourceId | None = None


class TaskAssignment(ConfigModel):
    model: ResourceId
    prompt: str | None = Field(default=None, min_length=1)
    operation: Literal["generation", "transcription"] = "generation"
    output_mode: Literal["text", "native", "tool"] = "text"
    preset: ResourceId | None = None
    fallback: ConfigSequence[Fallback] = Field(default_factory=tuple)
    budget: Budget = Field(default_factory=Budget)

    @model_validator(mode="after")
    def generation_prompt_required(self) -> TaskAssignment:
        if self.operation == "generation" and self.prompt is None:
            raise ValueError("Generation tasks require a prompt resource")
        return self


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
    modalities: ConfigSequence[Literal["text", "image", "audio", "video"]] = Field(min_length=1)
    normalization: Literal["none", "l2"]
    encoding: EncodingConfig


class EmbeddingConfig(ConfigModel):
    max_batch_size: int = setting(
        64, ge=1, description="Maximum inputs submitted in one embedding gateway request."
    )

    spaces: ConfigMap[ResourceId, EmbeddingSpace] = setting(
        {},
        apply="new_binding",
        impact="data_bound",
        level="standard",
        description="A space fixes model deployment, dimensions and input encoding; used spaces require a new ID when these change.",
    )
