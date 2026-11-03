"""Shared attempt classification, error mapping and deadlines for model endpoints."""

from __future__ import annotations

import asyncio
import json
from typing import TYPE_CHECKING, cast

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

if TYPE_CHECKING:
    from nous.runtime.execution import OperationContext


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
    elif isinstance(error, ModelAPIError):
        service_error["message"] = error.message
    return NousError(
        "model.request_failed",
        "The model request failed",
        details={
            "error_type": type(error).__name__,
            "status_code": status,
            "attempts": attempts,
            "request_id": getattr(error, "request_id", None),
            "provider_error": service_error,
        },
        retryable=retryable(error),
        cause=error,
    )


async def retry_delay(error: BaseException) -> None:
    delay = 0.25
    if isinstance(error, ModelAPIError) and error.retry_after is not None:
        delay = max(0, error.retry_after)
    elif isinstance(error, APIStatusError):
        header = error.response.headers.get("retry-after")
        if header:
            try:
                delay = max(0, float(header))
            except ValueError:
                pass
    await asyncio.sleep(delay)


def stage_deadline(seconds: float, context: OperationContext | None) -> float:
    """Include attempts and retry waits in one deadline, bounded by the root operation."""
    deadline = asyncio.get_running_loop().time() + seconds
    return min(deadline, context.deadline) if context and context.deadline is not None else deadline
