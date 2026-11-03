"""Shared retention state and exact lifecycle operations for memories and associations."""

from typing import Literal

from pydantic import Field, model_validator

from .cognitive_identity import CognitiveRef
from .common import Contract, Operation

RetentionAction = Literal[
    "reinforce",
    "weaken",
    "protect",
    "unprotect",
    "pin",
    "unpin",
    "freeze",
    "forget",
    "restore",
    "trash",
    "purge",
    "decay",
    "remember_forever",
    "allow_decay",
]


class RetentionState(Contract):
    strength: float = Field(default=1, ge=0, le=1, allow_inf_nan=False)
    anchor_unix_ms: int
    activation: Literal["active", "frozen", "forgotten", "weakened", "decayed"] = "active"
    pinned: bool = False
    permanent: bool = Field(default=False, exclude_if=lambda value: not value)
    protected_until_unix_ms: int | None = None
    inactive_since_unix_ms: int | None = None
    last_reinforced_unix_ms: int | None = None

    def protected(self, now_unix_ms: int) -> bool:
        return (
            self.permanent
            or self.pinned
            or (
                self.protected_until_unix_ms is not None
                and self.protected_until_unix_ms > now_unix_ms
            )
        )


class RetentionChange(Operation):
    reference: CognitiveRef
    action: RetentionAction
    strength: float = Field(default=1, ge=0, allow_inf_nan=False)
    hours: float | None = Field(default=None, ge=-1000000, le=1000000, allow_inf_nan=False)
    reason: str = Field(default="", max_length=4096)
    force: bool = False

    @model_validator(mode="after")
    def retained_kind(self) -> "RetentionChange":
        if self.reference.kind not in {"memory", "association"}:
            raise ValueError("retention applies to Memory and Association")
        return self


class RetentionChanges(Operation):
    references: tuple[CognitiveRef, ...] = Field(min_length=1, max_length=1000)
    action: RetentionAction
    strength: float = Field(default=1, ge=0, allow_inf_nan=False)
    hours: float | None = Field(default=None, ge=-1000000, le=1000000, allow_inf_nan=False)
    reason: str = Field(default="", max_length=4096)
    force: bool = False


class RetentionExpectation(Contract):
    # Native content and lifecycle share this real committed object revision.
    reference: CognitiveRef
    strength: float = Field(ge=0, le=1, allow_inf_nan=False)
    anchor_unix_ms: int
    inactive_since_unix_ms: int | None
    inactive_reason: str
    inactive: bool
    permanent: bool
    pinned: bool
    protected_until_unix_ms: int | None


class RetentionSnapshot(RetentionExpectation):
    score: float = Field(ge=0, le=1, allow_inf_nan=False)


class DecayRetention(Operation):
    kinds: tuple[Literal["memory", "association"], ...] = ("memory", "association")
    limit: int = Field(default=1000, ge=1, le=1000)
    cursor: str | None = None


class RetentionDecayResult(Contract):
    scanned: int
    changed: tuple[CognitiveRef, ...]
    next_cursor: str | None = None
    complete: bool
