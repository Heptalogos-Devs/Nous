"""Typed instance configuration; secrets remain references."""

from pathlib import Path
from typing import Annotated, Literal

from nous_sdk.contracts.media_policy import MediaPolicy
from pydantic import AfterValidator, BaseModel, Field, JsonValue
from pydantic_settings import BaseSettings, SettingsConfigDict

from nous.platform.network.policy import NetworkPolicy

ReasoningEffort = Literal["none", "minimal", "low", "medium", "high", "xhigh"]


def provider_options(value: dict[str, JsonValue]) -> dict[str, JsonValue]:
    owned = {
        "model",
        "messages",
        "input",
        "response_format",
        "tools",
        "tool_choice",
        "max_tokens",
        "max_completion_tokens",
        "temperature",
        "reasoning_effort",
        "stream",
        "stream_options",
        "dimensions",
        "encoding_format",
    }
    if conflicts := owned.intersection(value):
        raise ValueError(
            "Provider options cannot replace owned request fields: " + ", ".join(sorted(conflicts))
        )
    return value


ProviderOptions = Annotated[dict[str, JsonValue], AfterValidator(provider_options)]


class ModelRoute(BaseModel):
    base_url: str = "https://api.openai.com/v1"
    model: str = ""
    credential_file: Path | None = None
    credential_env: str = "NOUS_MODEL_API_KEY"
    timeout_seconds: float = Field(default=120, gt=0)
    max_output_tokens: int = Field(default=4096, gt=0)
    temperature: float | None = Field(default=None, ge=0, le=2)
    reasoning_effort: ReasoningEffort | None = None
    max_calls_per_day: int = Field(default=50, ge=0)
    video_content_format: Literal["video_url"] | None = None
    extra_body: ProviderOptions = Field(default_factory=dict)


class EmbeddingRoute(ModelRoute):
    model: str = ""
    dimensions: int = Field(default=2048, gt=0)
    normalization: Literal["none", "l2"] = "none"
    media_path: str = "embeddings/multimodal"
    max_batch_items: int = Field(default=64, ge=1, le=2048)


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_prefix="NOUS_", env_nested_delimiter="__")
    instance: Path = Path("temp/instances/default")
    host: str = "127.0.0.1"
    port: int = Field(default=7441, ge=1, le=65535)
    locale: str = "zh-CN"
    model: ModelRoute = Field(default_factory=ModelRoute)
    embedding: EmbeddingRoute = Field(default_factory=EmbeddingRoute)
    network: NetworkPolicy = Field(default_factory=NetworkPolicy)
    media: MediaPolicy = Field(default_factory=MediaPolicy)
    log_level: str = "INFO"

    @property
    def database(self) -> Path:
        return self.instance / "data" / "authority.sqlite"

    @property
    def token_file(self) -> Path:
        return self.instance / "credentials" / "admin.token"
