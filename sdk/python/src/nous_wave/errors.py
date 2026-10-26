from collections.abc import Mapping
from typing import Any


class NousError(Exception):
    def __init__(
        self, detail: Mapping[str, Any], *, status_code: int | None = None
    ) -> None:
        self.detail = dict(detail)
        self.code = str(detail.get("code", "DependencyUnavailable"))
        self.trace_id = detail.get("trace_id")
        self.context = dict(detail.get("context") or {})
        self.detail["context"] = self.context
        self.status_code = status_code
        super().__init__(str(detail.get("message", "Nous request failed")))
