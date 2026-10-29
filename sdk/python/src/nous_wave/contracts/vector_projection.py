"""Explicit automatic projection policy and durable admission receipts."""

from typing import Literal

from pydantic import Field

from .cognition import CognitiveRef
from .common import Contract, Operation
from .media import MediaReference


class VectorProjectionPolicy(Contract):
    mode: Literal["disabled", "changed_heads"] = "disabled"


class ProjectionReceipt(Contract):
    request_id: str
    job_id: str


class ProjectVectors(Operation):
    targets: tuple[CognitiveRef | MediaReference, ...] = Field(min_length=1, max_length=10000)


class ProjectionResult(Contract):
    request_id: str
    references: tuple[str, ...] = ()
    omitted: tuple[str, ...] = ()
    spaces: tuple[str, ...] = ()
    call_ids: tuple[str, ...] = ()
