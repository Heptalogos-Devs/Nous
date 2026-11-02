"""Actual optional model reranking status on the shared query result."""

from typing import Literal

from pydantic import JsonValue

from .common import Contract
from .common import ModelProvenance


class RerankInfo(Contract):
    algorithm: Literal["off", "compute", "cross_encoder", "llm"] = "off"
    before: tuple[str, ...] = ()
    after: tuple[str, ...] = ()
    mode: Literal["off", "optional", "required"]
    status: Literal["skipped", "completed"]
    producer: ModelProvenance | None = None
    usage: dict[str, JsonValue] | None = None
    scored_documents: int = 0
    retained_base_order: int = 0
    reason: str | None = None
