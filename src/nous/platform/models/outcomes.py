"""Classify known transmission outcomes without interpreting human error messages."""

import httpx
import httpx2
from nous_sdk.contracts import DomainError
from nous_sdk.contracts.model_routing import ModelCallOutcome, ModelRoutingPolicy
from openai import APIStatusError
from pydantic import JsonValue, TypeAdapter
from pydantic_ai.exceptions import ModelHTTPError


def outcome(error: Exception, *, response_received: bool = False) -> ModelCallOutcome:
    seen: set[int] = set()
    current: BaseException | None = error
    while current is not None and id(current) not in seen:
        seen.add(id(current))
        if isinstance(
            current,
            (
                httpx.ConnectError,
                httpx.ConnectTimeout,
                httpx.PoolTimeout,
                httpx2.ConnectError,
                httpx2.ConnectTimeout,
                httpx2.PoolTimeout,
            ),
        ):
            return ModelCallOutcome(kind="not_sent")
        if isinstance(current, (ModelHTTPError, APIStatusError)):
            if isinstance(current, ModelHTTPError) and current.in_stream:
                return ModelCallOutcome(kind="unknown", http_status=current.status_code)
            body = current.body
            code = parameter = provider_type = None
            if isinstance(body, dict):
                parsed = TypeAdapter(dict[str, JsonValue]).validate_python(body)
                inner = parsed.get("error")
                detail = inner if isinstance(inner, dict) else parsed
                raw_code, raw_parameter = detail.get("code"), detail.get("param")
                raw_type = detail.get("type")
                code = raw_code if isinstance(raw_code, str) else None
                parameter = raw_parameter if isinstance(raw_parameter, str) else None
                provider_type = raw_type if isinstance(raw_type, str) else None
            return ModelCallOutcome(
                kind="unknown"
                if current.status_code >= 500 or current.status_code == 409
                else "rejected",
                http_status=current.status_code,
                provider_code=code,
                parameter=parameter,
                provider_type=provider_type,
            )
        current = current.__cause__
    return ModelCallOutcome(
        kind="invalid_output"
        if response_received
        or isinstance(error, DomainError)
        and error.code == "model_output_incomplete"
        else "unknown"
    )


def may_continue(policy: ModelRoutingPolicy, value: ModelCallOutcome | None) -> bool:
    if value is None:
        return False
    if value.kind == "not_sent":
        return policy.not_sent
    if value.kind != "rejected":
        return False
    blocked = {
        "content_policy_violation",
        "content_filter",
        "safety_violation",
        "moderation_blocked",
        "refusal",
        "safety",
    }
    if value.provider_code in blocked or value.provider_type in blocked:
        return False
    if value.http_status in policy.rejected_statuses:
        return True
    return (
        policy.unsupported_request
        and value.http_status == 400
        and (
            value.provider_code
            in {
                "unsupported_parameter",
                "model_not_found",
                "context_length_exceeded",
                "invalid_response_format",
            }
            or value.provider_code == "invalid_request_error"
            and value.parameter == "response_format"
        )
    )
