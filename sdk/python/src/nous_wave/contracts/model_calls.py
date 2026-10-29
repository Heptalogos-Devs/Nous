"""Retained model-call facts exchanged by native knowledge packages."""

from __future__ import annotations

from typing import Literal

from pydantic import Field, JsonValue

from .common import Contract


class CallOrigin(Contract):
    namespace_id: str
    subject_id: str | None
    call_id: str


class ArchivedCall(Contract):
    id: str
    origin: CallOrigin
    purpose: str
    request_hash: str
    day: str
    status: Literal["reserved", "succeeded", "failed", "purged"]
    request: dict[str, JsonValue]
    result: JsonValue = None
    usage: dict[str, JsonValue] | None = None
    error_code: str | None = None
    source_ids: tuple[str, ...] = Field(default=())
