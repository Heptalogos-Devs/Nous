"""Actual optional model reranking status on the shared query result."""

from typing import Literal

from .common import Contract


class RerankInfo(Contract):
    algorithm: Literal["off", "compute", "cross_encoder", "llm"] = "off"
    before: tuple[str, ...] = ()
    after: tuple[str, ...] = ()
    mode: Literal["off", "optional", "required"]
    status: Literal["skipped", "completed"]
    call_id: str | None = None
    invocation_id: str | None = None
    scored_documents: int = 0
    retained_base_order: int = 0
    reason: str | None = None
