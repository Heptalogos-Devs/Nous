"""Transport-independent failures shared by local application operations."""

import asyncio
from collections.abc import Mapping

from pydantic import JsonValue, TypeAdapter, ValidationError


def failure_code(error: BaseException) -> str:
    if isinstance(error, NousError):
        return error.code
    if isinstance(error, (asyncio.CancelledError, KeyboardInterrupt)):
        return "operation.cancelled"
    if isinstance(error, TimeoutError):
        return "operation.timeout"
    if isinstance(error, ValueError):
        return "input.invalid"
    return "operation.failed"


def failure_message(error: BaseException) -> str:
    if isinstance(error, ValidationError):
        return "; ".join(
            ".".join(map(str, issue["loc"])) + ": " + issue["msg"]
            for issue in error.errors(include_input=False, include_url=False)
        )
    return str(error) or (
        "Operation exceeded its deadline."
        if isinstance(error, TimeoutError)
        else "Operation cancelled."
        if isinstance(error, (asyncio.CancelledError, KeyboardInterrupt))
        else type(error).__name__
    )


class NousError(Exception):
    def __init__(
        self,
        code: str,
        message: str,
        *,
        operation: str | None = None,
        details: Mapping[str, object] | None = None,
        cause: BaseException | None = None,
        retryable: bool = False,
        retry_after: float | None = None,
        operation_id: str | None = None,
        trace_id: str | None = None,
    ) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.operation = operation
        self.details = TypeAdapter(dict[str, JsonValue]).validate_python(dict(details or {}))
        self.retryable = retryable
        self.retry_after = retry_after
        self.operation_id = operation_id
        self.trace_id = trace_id
        self.__cause__ = cause

    def to_dict(self) -> dict[str, JsonValue]:
        result: dict[str, JsonValue] = {
            "code": self.code,
            "message": self.message,
            "details": dict(self.details),
            "retryable": self.retryable,
        }
        for name in ("operation", "retry_after", "operation_id", "trace_id"):
            value = getattr(self, name)
            if value is not None:
                result[name] = value
        return result
