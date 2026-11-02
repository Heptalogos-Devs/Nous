"""Public failures, mapped once at each transport boundary."""

from collections.abc import Mapping

from pydantic import JsonValue, TypeAdapter


class DomainError(Exception):
    def __init__(self, code: str, message: str, **details: object) -> None:
        super().__init__(message)
        self.code = code
        self.details = TypeAdapter(dict[str, JsonValue]).validate_python(details)


class NousError(Exception):
    def __init__(
        self,
        code: str,
        message: str,
        *,
        status: int = 400,
        context: Mapping[str, JsonValue] | None = None,
    ) -> None:
        super().__init__(message)
        self.code = code
        self.status = status
        self.context = TypeAdapter(dict[str, JsonValue]).validate_python(dict(context or {}))

    @property
    def data(self) -> dict[str, JsonValue]:
        """Expose the same context to DBOS's portable JSON exception serializer."""
        return self.context


class Unavailable(NousError):
    def __init__(self, message: str, *, context: Mapping[str, JsonValue] | None = None) -> None:
        super().__init__("Unavailable", message, status=503, context=context)
