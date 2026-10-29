"""Inspect the registered consumer contract and its actual invocation without a model call."""

from __future__ import annotations

from typing import Literal

from pydantic import ConfigDict, Field, JsonValue

from .common import Contract
from .media import MediaReference


class ModelContractRequest(Contract):
    role: str = Field(min_length=1)
    variant: str | None = None


class ModelPreviewRequest(ModelContractRequest):
    input: dict[str, JsonValue]
    media: tuple[MediaReference, ...] = ()
    source_ids: tuple[str, ...] = ()


class ModelRoleContract(Contract):
    role: str
    purpose: str
    variant: str
    variants: tuple[str, ...]
    input_schema: dict[str, JsonValue]
    output_schema: dict[str, JsonValue]
    prompt_id: str
    prompt: ModelPrompt
    provider_called: Literal[False] = False


class ModelPrompt(Contract):
    id: str
    revision: int
    template: str
    active: bool
    source: Literal["bundled", "instance"]
    variable_names: tuple[str, ...]
    template_hash: str
    variables: dict[str, JsonValue] | None = None
    rendered: str | None = None
    hash: str | None = None
    description_call_id: str | None = None


class InvocationParameters(Contract):
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


class InvocationSnapshot(Contract):
    model_config = ConfigDict(extra="forbid", frozen=True, serialize_by_alias=True)
    purpose: str
    source_ids: tuple[str, ...]
    model: str
    base_url: str
    parameters: InvocationParameters
    prompt: ModelPrompt
    output_schema: dict[str, JsonValue] = Field(alias="schema")
    media: tuple[InvocationMedia, ...]
    video_content_format: Literal["video_url"] | None = None


class ModelInvocationPreview(Contract):
    contract: ModelRoleContract
    invocation: InvocationSnapshot
    capabilities: tuple[str, ...]
    configured: bool
    provider_called: Literal[False] = False


ModelRoleContract.model_rebuild()
