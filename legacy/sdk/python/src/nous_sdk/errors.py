from collections.abc import Mapping

from pydantic import JsonValue

from .types import ErrorDetail


class NousError(Exception):
    def __init__(
        self, detail: ErrorDetail | Mapping[str, JsonValue], *, status_code: int | None = None
    ) -> None:
        parsed = detail if isinstance(detail, ErrorDetail) else ErrorDetail.model_validate(detail)
        self.detail = parsed
        self.code = parsed.code
        self.trace_id = parsed.trace_id
        self.context = dict(parsed.context)
        self.status_code = status_code
        super().__init__(parsed.message)
