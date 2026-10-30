"""Actual optional model reranking status on the shared query result."""

from typing import Literal

from .common import Contract


class RerankInfo(Contract):
    mode: Literal["off", "optional", "required"]
    status: Literal["skipped", "completed"]
    call_id: str | None = None
    invocation_id: str | None = None
    scored_documents: int = 0
    retained_base_order: int = 0
    reason: str | None = None
