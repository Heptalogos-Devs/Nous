"""Typed instance configuration; secrets remain references."""

import hashlib
import json
from pathlib import Path
from typing import Annotated, Literal

from pydantic import AfterValidator, BaseModel, Field, JsonValue
from pydantic_settings import BaseSettings, SettingsConfigDict

from nous.platform.paths import default_instance_path

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
    tokenizer: str | None = None
    resource_request_budget: int = Field(default=50, ge=0)

    @property
    def budget_resource(self) -> str:
        return hashlib.sha256(
            json.dumps(
                [
                    self.base_url,
                    self.model,
                    self.credential_env,
                    str(self.credential_file) if self.credential_file else None,
                ],
                separators=(",", ":"),
            ).encode()
        ).hexdigest()

    protocol: Literal["openai-chat", "openai-responses"] = "openai-chat"
    input_modalities: tuple[str, ...] = ("text",)
    tools: bool = False
    media_limits: dict[str, int] = Field(default_factory=dict)
    context_window: int = Field(default=32768, gt=0)
    structured_output_mode: Literal["native", "tool", "text"] = "native"
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


class RPCSettings(BaseModel):
    enabled: bool = False
    host: str = "127.0.0.1"
    port: int = Field(default=7442, ge=0, le=65535)
    maximum_message_bytes: int = Field(default=16 * 1024 * 1024, ge=1024, le=256 * 1024 * 1024)


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_prefix="NOUS_", env_nested_delimiter="__")
    instance: Path = Field(default_factory=default_instance_path)
    host: str = "127.0.0.1"
    port: int = Field(default=7441, ge=1, le=65535)
    locale: str = "zh-CN"
    rpc: RPCSettings = Field(default_factory=RPCSettings)
    log_level: str = "INFO"

    @property
    def database(self) -> Path:
        return self.instance / "data" / "authority.sqlite"

    @property
    def token_file(self) -> Path:
        return self.instance / "credentials" / "admin.token"
