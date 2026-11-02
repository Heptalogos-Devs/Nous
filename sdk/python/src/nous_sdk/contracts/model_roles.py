"""Inspect the registered consumer contract and its actual invocation without a model call."""

from __future__ import annotations

from typing import Literal

from pydantic import ConfigDict, Field, JsonValue, model_validator

from .common import Contract
from .media import MediaInfo, MediaReference
from .prompt_compilation import CompiledPrompt, PromptMessage

ModelCachePolicy = Literal["none", "shared"]


class ModelContractRequest(Contract):
    role: str = Field(min_length=1)
    variant: str | None = None


class ModelPreviewRequest(ModelContractRequest):
    input: dict[str, JsonValue]
    media: tuple[MediaReference, ...] = ()
    source_ids: tuple[str, ...] = ()
    draft_template: str | None = None
    context_messages: tuple[PromptMessage, ...] = ()
    max_output_tokens: int | None = Field(default=None, ge=1)


class ModelRoleContract(Contract):
    role: str
    purpose: str
    variant: str
    variants: tuple[str, ...]
    input_schema: dict[str, JsonValue]
    output_schema: dict[str, JsonValue]
    output_mode: Literal["text", "json_schema"]
    provider_response_format: dict[str, JsonValue] | None
    template_id: str
    prompt: ModelPrompt
    provider_called: Literal[False] = False


class ModelPrompt(Contract):
    id: str
    revision: int | None
    template: str
    active: bool
    source: Literal["bundled", "instance", "draft"]
    variable_names: tuple[str, ...]
    template_hash: str
    variables: dict[str, JsonValue] | None = None
    rendered: str | None = None
    hash: str | None = None

    @model_validator(mode="after")
    def revision_identity(self) -> ModelPrompt:
        if self.source == "draft":
            if self.revision is not None or self.active:
                raise ValueError("A draft prompt has no revision and cannot be active")
        elif self.revision is None:
            raise ValueError("A saved or bundled prompt requires its actual revision")
        return self


class InvocationParameters(Contract):
    timeout_seconds: float
    max_output_tokens: int
    temperature: float | None
    reasoning_effort: str | None
    extra_body: dict[str, JsonValue]


class InvocationMedia(Contract):
    sha256: str
    byte_len: int
    media_type: str
    source_reference: str
    channel: str | None = None
    position: int
    ordinal: int
    media_info: MediaInfo | None = None


class InvocationSnapshot(Contract):
    model_config = ConfigDict(extra="forbid", frozen=True, serialize_by_alias=True)
    purpose: str
    source_ids: tuple[str, ...]
    model: str
    base_url: str
    protocol: Literal["openai-chat", "openai-responses", "volcengine-ark"]
    parameters: InvocationParameters
    prompt: CompiledPrompt
    output_schema: dict[str, JsonValue] = Field(alias="schema")
    output_mode: Literal["text", "json_schema"]
    provider_response_format: dict[str, JsonValue] | None
    media: tuple[InvocationMedia, ...]


class ModelInvocationPreview(Contract):
    contract: ModelRoleContract
    invocation: InvocationSnapshot | None
    capabilities: tuple[str, ...]
    configured: bool
    provider_called: Literal[False] = False


ModelRoleContract.model_rebuild()
