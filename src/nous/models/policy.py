"""Shared attempt classification, error mapping and deadlines for model endpoints."""

from __future__ import annotations

import asyncio
import contextvars
import json
import math
from collections.abc import Generator, Mapping
from contextlib import contextmanager
from datetime import UTC, datetime
from email.utils import parsedate_to_datetime
from typing import TYPE_CHECKING, TypedDict, cast
from urllib.parse import parse_qsl, urlsplit

from openai import APIConnectionError, APIStatusError, APITimeoutError
from pydantic import JsonValue
from pydantic_ai.exceptions import (
    ModelAPIError,
    ModelConnectionError,
    ModelHTTPError,
    ModelQuotaExceededError,
    ModelRateLimitError,
    ModelRequestTimeout,
    ModelServerError,
    ModelUnavailableError,
)

from nous.common.errors import NousError
from nous.observability.instrumentation import AttemptScope, request_attempt, request_attempts

if TYPE_CHECKING:
    from nous.configuration import ConfigSnapshot
    from nous.runtime.execution import OperationContext


_active_attempt: contextvars.ContextVar[tuple[Mapping[str, JsonValue], str] | None] = (
    contextvars.ContextVar("model_attempt_route", default=None)
)


def endpoint_path(snapshot: ConfigSnapshot, provider: str, endpoint: str) -> str:
    """Describe the actual configured API path without query strings or credentials."""
    base = urlsplit(snapshot.values.providers[provider].endpoint).path.rstrip("/")
    return base + "/" + endpoint.lstrip("/")


def _retry_after(error: BaseException) -> float | None:
    if isinstance(error, ModelAPIError) and error.retry_after is not None:
        value = float(error.retry_after)
        return max(0.0, value) if math.isfinite(value) else None
    header: str | None = None
    if isinstance(error, APIStatusError):
        header = error.response.headers.get("retry-after")
    elif isinstance(error, ModelHTTPError) and error.headers:
        header = error.headers.get("retry-after")
    if not header:
        if isinstance(error, (APIStatusError, ModelHTTPError)) and isinstance(error.body, dict):
            body = cast(dict[str, object], error.body)
            nested = body.get("error")
            if isinstance(nested, dict):
                body = cast(dict[str, object], nested)
            value = body.get("retry_after", body.get("retryAfter"))
            if isinstance(value, (int, float, str)) and not isinstance(value, bool):
                header = str(value)
        if not header:
            return None
    try:
        seconds = float(header)
    except ValueError:
        try:
            date = parsedate_to_datetime(header)
            seconds = (date - datetime.now(UTC)).total_seconds()
        except TypeError, ValueError, OverflowError:
            return None
    return max(0.0, seconds) if math.isfinite(seconds) else None


def _request_facts(error: BaseException) -> tuple[str | None, str | None, str | None]:
    method = path = request_id = None
    if isinstance(error, (APIConnectionError, APIStatusError)):
        method, path = error.request.method, error.request.url.path
        request_id = getattr(error, "request_id", None)
    if isinstance(error, ModelHTTPError) and error.headers:
        request_id = error.headers.get("x-request-id") or error.headers.get("request-id")
    return method, path, request_id


def _safe_provider_text(value: str, error: BaseException) -> str:
    """Avoid echoing request credentials if a service includes them in its error text."""
    if not isinstance(error, (APIConnectionError, APIStatusError)):
        return value
    request = error.request
    secrets: set[str] = set()
    for name in ("authorization", "proxy-authorization", "api-key", "x-api-key", "x-goog-api-key"):
        header = request.headers.get(name)
        if header:
            secrets.add(header)
            scheme, separator, credential = header.partition(" ")
            if separator and scheme.lower() in {"bearer", "basic"}:
                secrets.add(credential)
    for key, secret in parse_qsl(urlsplit(str(request.url)).query, keep_blank_values=True):
        if any(
            word in key.lower()
            for word in ("key", "token", "secret", "password", "auth", "signature", "credential")
        ):
            secrets.add(secret)
    for secret in sorted(secrets, key=len, reverse=True):
        if len(secret) >= 4:
            value = value.replace(secret, "[redacted]")
    return value


def retryable(error: BaseException) -> bool:
    if isinstance(error, ModelQuotaExceededError):
        return False
    if isinstance(
        error,
        (
            APIConnectionError,
            APITimeoutError,
            TimeoutError,
            ModelConnectionError,
            ModelRequestTimeout,
            ModelRateLimitError,
            ModelUnavailableError,
            ModelServerError,
        ),
    ):
        return True
    if isinstance(error, (APIStatusError, ModelHTTPError)):
        return error.status_code in {408, 409, 429} or error.status_code >= 500
    return False


def route_unavailable(error: NousError) -> bool:
    """Skip an unusable representation or transient prerequisite without counting a chat call."""
    return (
        error.code
        in {
            "media.format_unavailable",
            "media.vision_required",
            "media.audio_capability_missing",
            "media.jpeg_background_required",
            "media.input_too_large",
            "model.joint_input_limit",
            "model.capability_missing",
            "model.protocol_format_unsupported",
            "model.protocol_unsupported",
            "model.input_limit",
        }
        or error.code == "model.request_failed"
        and error.retryable
    )


def model_error(error: BaseException, attempts: int) -> NousError:
    if isinstance(error, NousError):
        return error
    status = error.status_code if isinstance(error, (APIStatusError, ModelHTTPError)) else None
    service_error: dict[str, JsonValue] = {}
    if isinstance(error, (APIStatusError, ModelHTTPError)):
        body: object = error.body
        if isinstance(body, str):
            try:
                body = cast(object, json.loads(body))
            except ValueError:
                service_error["message"] = body
        if isinstance(body, dict):
            values = cast(dict[str, object], body)
            nested = values.get("error")
            if isinstance(nested, dict):
                values = cast(dict[str, object], nested)
            for field in ("message", "type", "code"):
                value = values.get(field)
                if isinstance(value, (str, int)):
                    service_error[field] = value
    if isinstance(error, ModelAPIError):
        if error.provider_error_code is not None:
            service_error.setdefault("code", error.provider_error_code)
        if error.provider_error_type is not None:
            service_error.setdefault("type", error.provider_error_type)
    if isinstance(error, ModelAPIError) and not isinstance(error, ModelHTTPError):
        service_error["message"] = error.message
    service_error = {
        key: _safe_provider_text(value, error) if isinstance(value, str) else value
        for key, value in service_error.items()
    }
    method, path, request_id = _request_facts(error)
    active = _active_attempt.get()
    route = dict(active[0]) if active else {}
    if path is None:
        recorded = route.get("endpoint_path")
        path = recorded if isinstance(recorded, str) else None
    provider_message = service_error.get("message")
    message = (
        f"Model request failed: {provider_message}"
        if isinstance(provider_message, str) and provider_message
        else f"Model request failed with HTTP {status}"
        if status is not None
        else "Model request failed"
    )
    return NousError(
        "model.request_failed",
        message,
        details={
            "error_type": type(error).__name__,
            "status_code": status,
            "attempt_count": attempts,
            "request_id": request_id,
            "method": method,
            "endpoint_path": path,
            "route": route,
            "task": active[1] if active else None,
            "provider_error": service_error,
        },
        retryable=retryable(error),
        retry_after=_retry_after(error),
        cause=error,
    )


async def retry_delay(error: BaseException) -> None:
    delay = _retry_after(error)
    await asyncio.sleep(delay if delay is not None else 0.25)


def stage_deadline(seconds: float, context: OperationContext | None) -> float:
    """Include attempts and retry waits in one deadline, bounded by the root operation."""
    deadline = asyncio.get_running_loop().time() + seconds
    return min(deadline, context.deadline) if context and context.deadline is not None else deadline


@contextmanager
def attempt_scope(
    number: int, route: Mapping[str, JsonValue], task: str
) -> Generator[AttemptScope]:
    """All endpoints submit real attempts through the same operation diagnostics owner."""
    token = _active_attempt.set((route, task))
    try:
        with request_attempt(number, route, task=task) as scope:
            yield scope
    finally:
        _active_attempt.reset(token)


def attempt_reports(start: int = 0) -> list[dict[str, JsonValue]]:
    return list(request_attempts()[start:])


class AttemptError(TypedDict):
    outcome: str
    error_type: str
    error_code: str
    provider_status: int | None
    provider_request_id: str | None
    provider_error: dict[str, JsonValue]
    endpoint_path: str | None
    retry_after: float | None
    error_message: str
    retry_reason: str | None
    next_action: str


def error_report(
    error: BaseException, *, next_action: str = "raise", repair: bool = False
) -> AttemptError:
    retry = next_action != "raise"
    mapped = model_error(error, 0)
    status = mapped.details.get("status_code")
    request_id = mapped.details.get("request_id")
    endpoint = mapped.details.get("endpoint_path")
    provider = mapped.details.get("provider_error")
    return {
        "outcome": "retry" if retry else "error",
        "error_type": type(error).__name__,
        "error_code": mapped.code,
        "provider_status": status if isinstance(status, int) else None,
        "provider_request_id": request_id if isinstance(request_id, str) else None,
        "provider_error": cast(dict[str, JsonValue], provider)
        if isinstance(provider, dict)
        else {},
        "endpoint_path": endpoint if isinstance(endpoint, str) else None,
        "retry_after": mapped.retry_after,
        "error_message": mapped.message,
        "retry_reason": "structured_output"
        if repair
        else "transient_service_error"
        if retry
        else None,
        "next_action": next_action,
    }
