"""Stable identity for an executable prepared request and its recovery context."""

from __future__ import annotations

import copy
import hashlib
import json
import math
from collections.abc import Awaitable, Callable, Mapping, Sequence
from dataclasses import dataclass, fields, is_dataclass
from datetime import date, datetime, time
from enum import Enum
from typing import TYPE_CHECKING, cast

from pydantic import BaseModel, JsonValue, TypeAdapter
from pydantic_ai.messages import (
    BinaryContent,
    ModelRequest,
    ModelResponse,
    SystemPromptPart,
    UserPromptPart,
)

from nous.common.errors import NousError
from nous.content.contracts import ArtifactRef, InputGroup, ResultRef, SelectedInputs
from nous.models.configuration import ModelRoute
from nous.models.requests import PreparedGeneration

from .computation import reading_value
from .contracts import schema_request_value

if TYPE_CHECKING:
    from nous.models.tasks import TaskDefinition
    from nous.runtime.execution import ExecutionManager


def _message_value(value: object) -> JsonValue:
    """Represent SDK messages without process-local timestamps or inline media bytes."""
    if value is None:
        return ["null"]
    if isinstance(value, bool):
        return ["bool", value]
    if isinstance(value, str):
        return ["string", value]
    if isinstance(value, int):
        return ["integer", value]
    if isinstance(value, float):
        return ["number", value]
    if isinstance(value, bytes):
        return ["bytes", len(value), hashlib.sha256(value).hexdigest()]
    if isinstance(value, BinaryContent):
        return [
            "binary-content",
            value.media_type,
            value.identifier,
            _message_value(value.data),
            _message_value(value.vendor_metadata),
        ]
    if isinstance(value, ModelRequest):
        return ["request", _message_value(value.instructions), _message_value(value.parts)]
    if isinstance(value, ModelResponse):
        return [
            "response",
            _message_value(value.provider_name),
            _message_value(value.canonical_system),
            _message_value(value.parts),
        ]
    if isinstance(value, Enum):
        return _message_value(value.value)
    if isinstance(value, datetime | date | time):
        return ["datetime", value.isoformat()]
    if isinstance(value, BaseModel):
        return [
            "model",
            f"{type(value).__module__}.{type(value).__qualname__}",
            _message_value(value.model_dump(mode="python")),
        ]
    if is_dataclass(value) and not isinstance(value, type):
        return [
            "dataclass",
            f"{type(value).__module__}.{type(value).__qualname__}",
            {
                item.name: _message_value(getattr(value, item.name))
                for item in fields(value)
                if not (
                    item.name == "timestamp"
                    and isinstance(value, SystemPromptPart | UserPromptPart)
                )
            },
        ]
    if isinstance(value, Mapping):
        mapping = cast(Mapping[object, object], value)
        if not all(isinstance(key, str) for key in mapping):
            raise TypeError("Model message identity requires string mapping keys")
        return ["mapping", {str(key): _message_value(item) for key, item in mapping.items()}]
    if isinstance(value, Sequence):
        return ["sequence", [_message_value(item) for item in cast(Sequence[object], value)]]
    raise TypeError(f"Unsupported model message identity value: {type(value).__qualname__}")


def _json_value(value: object) -> JsonValue:
    """Normalize Python sequences to the JSON representation retained on disk."""
    if isinstance(value, Mapping):
        mapping = cast(Mapping[object, object], value)
        if not all(isinstance(key, str) for key in mapping):
            raise TypeError("Coverage requires string mapping keys")
        return {str(key): _json_value(item) for key, item in mapping.items()}
    if isinstance(value, (list, tuple)):
        return [_json_value(item) for item in cast(Sequence[object], value)]
    if isinstance(value, float) and not math.isfinite(value):
        raise ValueError("Coverage requires finite JSON numbers")
    if value is None or isinstance(value, (str, bool, int, float)):
        return value
    raise TypeError(f"Unsupported coverage value: {type(value).__qualname__}")


@dataclass(frozen=True)
class CapturedPrepared:
    """One private request value shared by identity, execution, and recovery."""

    prepared: PreparedGeneration
    inputs: tuple[InputGroup, ...] | None
    coverage: dict[str, object] | None
    selected_inputs: SelectedInputs | None
    dependencies: tuple[ArtifactRef | ResultRef, ...]
    frozen_task_ref: ArtifactRef | None
    identity: str


@dataclass(frozen=True)
class PreparedInvocation:
    captured: CapturedPrepared
    prepare_route: Callable[[ModelRoute], Awaitable[PreparedGeneration]] | None


async def capture_invocation(
    execution: ExecutionManager,
    prepared: PreparedGeneration,
    *,
    definition: TaskDefinition,
    producer_key: str | None,
    caller_identity: str | None,
    prepare_route: Callable[[ModelRoute], Awaitable[PreparedGeneration]] | None,
    inputs: Sequence[InputGroup] | None,
    coverage: Mapping[str, object] | None,
    selected_inputs: SelectedInputs | None,
    dependencies: Sequence[ArtifactRef | ResultRef],
    frozen_task_ref: ArtifactRef | None,
) -> PreparedInvocation:
    """Validate and capture the execution/recovery boundary for a keyed request."""
    deferred_routes = prepare_route is not None and len(prepared.routes) > 1
    if producer_key is not None and caller_identity is None:
        if deferred_routes:
            raise NousError(
                "model.prepared_identity_required",
                "A keyed deferred route callback requires an identity for its inputs and behavior",
            )
        if prepared.compiled.validate_output != definition.validate:
            raise NousError(
                "model.prepared_identity_required",
                "A keyed custom validator requires an identity for its behavior",
            )
    captured = await execution.blocking(
        capture_prepared,
        prepared,
        execution_contract=definition.execution_contract(),
        caller_identity=caller_identity,
        deferred_routes=deferred_routes,
        inputs=inputs,
        coverage=coverage,
        selected_inputs=selected_inputs,
        dependencies=dependencies,
        frozen_task_ref=frozen_task_ref,
    )
    if prepare_route is None:
        return PreparedInvocation(captured, None)

    async def capture_route(route: ModelRoute) -> PreparedGeneration:
        fallback = await prepare_route(route)
        return await execution.blocking(copy_prepared, fallback)

    return PreparedInvocation(captured, capture_route)


def _copy_memo(prepared: PreparedGeneration) -> dict[int, object]:
    # These resources are deeply immutable and may contain MappingProxyType;
    # preserve them while copying mutable messages, reading data and coverage.
    immutable = (
        prepared.snapshot,
        prepared.assignment,
        prepared.compiled.resource,
        prepared.compiled.validate_output,
        *prepared.routes,
        *(attempt.route for attempt in prepared.attempt_plans),
        *(attempt.compiled.resource for attempt in prepared.attempt_plans),
        *(attempt.compiled.validate_output for attempt in prepared.attempt_plans),
    )
    return {id(value): value for value in immutable}


def copy_prepared(prepared: PreparedGeneration) -> PreparedGeneration:
    """Capture one route without copying immutable binary buffers or configuration."""
    return copy.deepcopy(prepared, _copy_memo(prepared))


def capture_prepared(
    prepared: PreparedGeneration,
    *,
    execution_contract: Mapping[str, JsonValue],
    caller_identity: str | None,
    deferred_routes: bool,
    inputs: Sequence[InputGroup] | None,
    coverage: Mapping[str, object] | None,
    selected_inputs: SelectedInputs | None,
    dependencies: Sequence[ArtifactRef | ResultRef],
    frozen_task_ref: ArtifactRef | None,
) -> CapturedPrepared:
    """Copy caller-owned mutable values before computing their identity."""
    normalized_coverage: dict[str, object] | None = None
    if coverage is not None:
        value = _json_value(coverage)
        assert isinstance(value, dict)
        normalized_coverage = cast(dict[str, object], value)
    (
        prepared_copy,
        inputs_copy,
        coverage_copy,
        selected_copy,
        dependencies_copy,
        task_copy,
    ) = copy.deepcopy(
        (
            prepared,
            tuple(inputs) if inputs is not None else None,
            normalized_coverage,
            selected_inputs,
            tuple(dependencies),
            frozen_task_ref,
        ),
        _copy_memo(prepared),
    )
    identity = prepared_identity(
        prepared_copy,
        execution_contract=execution_contract,
        caller_identity=caller_identity,
        deferred_routes=deferred_routes,
        inputs=inputs_copy,
        coverage=coverage_copy,
        selected_inputs=selected_copy,
        dependencies=dependencies_copy,
        frozen_task_ref=task_copy,
    )
    return CapturedPrepared(
        prepared_copy,
        inputs_copy,
        coverage_copy,
        selected_copy,
        dependencies_copy,
        task_copy,
        identity,
    )


def prepared_identity(
    prepared: PreparedGeneration,
    *,
    execution_contract: Mapping[str, JsonValue],
    caller_identity: str | None,
    deferred_routes: bool,
    inputs: Sequence[InputGroup] | None,
    coverage: Mapping[str, object] | None,
    selected_inputs: SelectedInputs | None,
    dependencies: Sequence[ArtifactRef | ResultRef],
    frozen_task_ref: ArtifactRef | None,
) -> str:
    """Hash actual execution and adoption facts, excluding inspection-only metadata.

    A caller-supplied identity describes additional work, including any deferred
    route callback. It augments this contract instead of replacing it. Credentials
    are represented by reference, never by their secret values.
    """
    compiled = prepared.compiled
    routes: list[dict[str, object]] = []
    for route in prepared.routes:
        provider = prepared.snapshot.values.providers[route.model.provider]
        network = prepared.snapshot.values.provider_network(route.model.provider)
        routes.append(
            {
                "model_id": route.model_id,
                "model": route.model.model_dump(mode="json"),
                "preset": route.preset.model_dump(mode="json"),
                "provider": {
                    "protocol": provider.protocol,
                    "endpoint": provider.endpoint,
                    "credential_ref": provider.credential_ref,
                },
                "transport_timeouts": {
                    "connect": network.connect_timeout_seconds,
                    "read": network.read_timeout_seconds,
                    "write": network.write_timeout_seconds,
                    "pool": network.pool_timeout_seconds,
                },
            }
        )
    requested_coverage = (
        TypeAdapter(dict[str, JsonValue]).validate_python(coverage)
        if coverage is not None
        else None
    )
    contract: dict[str, object] = {
        "identity_contract": "prepared-generation-v1",
        "caller_identity": caller_identity,
        "deferred_routes": deferred_routes,
        "task": prepared.task,
        "operation": prepared.assignment.operation,
        "output_mode": prepared.assignment.output_mode,
        "budget_output_tokens": prepared.assignment.budget.max_output_tokens,
        "prompt_id": compiled.resource.id,
        "prompt_execution_signature": compiled.resource.execution_signature,
        "execution_contract": dict(execution_contract),
        "messages": _message_value(compiled.messages),
        "schema": schema_request_value(compiled.schema),
        "routes": routes,
        "max_attempts": prepared.max_attempts,
        "timeout_seconds": prepared.timeout_seconds,
        "max_output_tokens": prepared.max_output_tokens,
        "structured_retries": prepared.structured_retries,
        "estimated_input_tokens": prepared.estimated_input_tokens,
        "reading": reading_value(compiled.reading),
        "prepared_coverage": prepared.coverage,
        "stage_refs": list(prepared.stage_refs),
        "requested_inputs": [item.model_dump(mode="json") for item in inputs]
        if inputs is not None
        else None,
        "requested_coverage": requested_coverage,
        "selected_inputs": selected_inputs.model_dump(mode="json") if selected_inputs else None,
        "dependencies": [item.model_dump(mode="json") for item in dependencies],
        "frozen_task": frozen_task_ref.model_dump(mode="json") if frozen_task_ref else None,
    }
    encoded = json.dumps(
        contract, sort_keys=True, ensure_ascii=False, separators=(",", ":"), allow_nan=False
    )
    return hashlib.sha256(encoded.encode("utf-8")).hexdigest()
