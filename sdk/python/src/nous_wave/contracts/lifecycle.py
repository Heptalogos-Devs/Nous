from typing import Literal

from pydantic import Field

from .cognition import CognitiveRef, MemoryBody
from .common import Operation

MemoryAction = Literal[
    "reinforce",
    "weaken",
    "protect",
    "unprotect",
    "freeze",
    "forget",
    "restore",
    "trash",
    "purge",
    "revise",
    "decay",
]


class MemoryChange(Operation):
    reference: CognitiveRef
    action: MemoryAction
    strength: float = Field(default=1, ge=0, allow_inf_nan=False)
    hours: float | None = Field(default=None, ge=-1000000, le=1000000, allow_inf_nan=False)
    reason: str = Field(default="", max_length=4096)
    force: bool = False
    body: MemoryBody | None = None
