"""The role messages and output contract bound for one model task."""

from typing import Literal

from pydantic import Field, JsonValue

from .common import Contract


class PromptMessage(Contract):
    role: Literal["system", "user", "assistant", "tool"]
    content: str


class TokenEstimate(Contract):
    text_tokens: int = Field(ge=0)
    media_tokens: int | None = Field(default=None, ge=0)
    method: str
    estimated: bool = True


class CompiledPrompt(Contract):
    messages: tuple[PromptMessage, ...]
    output_schema: dict[str, JsonValue] | None = None
    template_id: str
    template_revision: int | None
    stable_prefix_fingerprint: str
    input_bindings: dict[str, JsonValue] = Field(default_factory=dict)
    model_options: dict[str, JsonValue] = Field(default_factory=dict)
    token_estimate: TokenEstimate
