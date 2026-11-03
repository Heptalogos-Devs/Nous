"""Transport-independent failures shared by local application operations."""

from collections.abc import Mapping

from pydantic import JsonValue, TypeAdapter


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
