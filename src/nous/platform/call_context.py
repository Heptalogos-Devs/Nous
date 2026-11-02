"""Trusted caller, Namespace, time and source scope for server operations."""

from __future__ import annotations

from uuid import uuid4

from nous_sdk.contracts.common import Contract, TimePoint, utc_now
from pydantic import Field


class CallContext(Contract):
    caller_id: str
    namespace_id: str
    request_id: str = Field(default_factory=lambda: str(uuid4()))
    locale: str = "en"
    clock: TimePoint = Field(default_factory=utc_now)
    credential_source_ids: tuple[str, ...] | None
    allowed_source_ids: tuple[str, ...] | None = None
    source_constraints: tuple[tuple[str, ...], ...] = ()

    def source_groups(self) -> tuple[tuple[str, ...], ...]:
        groups = self.source_constraints or (
            () if self.allowed_source_ids is None else (self.allowed_source_ids,)
        )
        if self.credential_source_ids is not None and self.credential_source_ids not in groups:
            groups = (*groups, self.credential_source_ids)
        return groups

    def for_source_group(self, group: tuple[str, ...]) -> CallContext:
        return self.model_copy(update={"allowed_source_ids": group, "source_constraints": ()})
