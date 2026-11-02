"""The persisted host runtime owns history, source keys and actual sending plans."""

from typing import Literal
from pydantic import Field, JsonValue
from .common import Contract
from .context import ContextProjection, ContextUsage, ContextTarget
from .media import MediaReference
from .model_measurement import ModelMeasurement
from .retrieval import QueryRequest

HostFormat = Literal["openai-chat", "pydantic-ai"]


class HostEvent(Contract):
    event_id: str = Field(min_length=1)
    role: Literal["system", "user", "assistant", "tool"]
    payload: JsonValue
    text: str = ""
    tool_call_id: str | None = None
    response_id: str | None = None


class RuntimeOpen(Contract):
    session: str = Field(min_length=1)
    instance: str = Field(default="main", min_length=1)
    consumer: str = "default"
    work: str | None = None
    usage: ContextUsage
    form_long_term: bool = True
    context_window_tokens: int | None = Field(default=None, gt=0)
    output_reserve: int = Field(default=0, ge=0)


class RuntimeBindingInfo(Contract):
    session: str
    instance: str
    consumer: str
    work: str | None
    usage: ContextUsage


class RuntimePrepare(Contract):
    binding: RuntimeBindingInfo
    invocation_id: str = Field(min_length=1)
    request: dict[str, JsonValue]
    format: HostFormat
    history_mode: Literal["full", "append"] = "full"
    events: tuple[HostEvent, ...] = ()
    query: QueryRequest | None = None
    trigger: Literal[
        "ordinary", "task_change", "tool_result", "conflict", "resume", "pause", "budget"
    ] = "ordinary"
    max_items: int = Field(default=64, ge=1, le=256)
    max_text_bytes: int = Field(default=32000, ge=1, le=1048576)


class RuntimeBlock(Contract):
    block_id: str
    role: Literal["system", "user"]
    segment_keys: tuple[str, ...]
    input_bindings: dict[str, ContextTarget]
    stable: bool


class RuntimePreparation(Contract):
    call_ref: str
    binding: RuntimeBindingInfo
    invocation_id: str
    projection: ContextProjection
    format: HostFormat
    request: dict[str, JsonValue]
    request_hash: str
    history_generation: str
    stable_prefix_fingerprint: str
    input_bindings: dict[str, ContextTarget]
    blocks: tuple[RuntimeBlock, ...]
    measurement: ModelMeasurement
    working_source: MediaReference | None = None
    replayed_output: JsonValue = None


class RuntimeSent(Contract):
    call_ref: str
    request_hash: str
    evidence: Literal["host_send_boundary", "response_observed", "response_started"] = (
        "host_send_boundary"
    )


class RuntimeComplete(Contract):
    call_ref: str
    outcome: Literal["completed", "failed", "cancelled", "unknown_outcome"]
    output: JsonValue = None
    text: str = ""
    model: str | None = None
    provider: str | None = None
    response_id: str | None = None
    usage: dict[str, JsonValue] | None = None
    error: dict[str, JsonValue] | None = None


class RuntimeResult(Contract):
    call_ref: str
    state: Literal["prepared", "sent", "completed", "failed", "cancelled", "unknown_outcome"]
    output_ref: MediaReference | None = None
    notices: tuple[str, ...] = ()


class RuntimeTool(Contract):
    binding: RuntimeBindingInfo
    event: HostEvent


class RuntimeRead(Contract):
    binding: RuntimeBindingInfo
    ref: str
    sources: bool = False
    max_chars: int = Field(default=12000, ge=1, le=1000000)
