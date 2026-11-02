"""Canonical record directory and exact reading contracts."""

from typing import Literal

from pydantic import Field

from .cognition import CognitiveKind, CognitiveRecord, CognitiveRef
from .common import Contract
from .retrieval import TimeFilter

RecordPurpose = Literal["current", "exact", "management"]


class RecordsQuery(Contract):
    
    kinds: tuple[CognitiveKind, ...] = ()
    object_ids: tuple[str, ...] = ()
    text: str | None = Field(default=None, max_length=16384)
    text_match: Literal["contains", "label"] = "contains"
    entity_ids: tuple[str, ...] = ()
    source_ids: tuple[str, ...] | None = None
    times: tuple[TimeFilter, ...] = ()
    view: Literal["current", "history"] = "current"
    purpose: RecordPurpose = "current"
    states: tuple[str, ...] = ()
    as_of_commit: int | None = Field(default=None, ge=0)
    cursor: str | None = Field(default=None, max_length=8192)
    limit: int = Field(default=100, ge=1, le=1000)


class RecordEntry(Contract):
    record: CognitiveRecord
    current_reference: CognitiveRef | None = None
    state: str


class RecordsCoverage(Contract):
    snapshot_commit: int
    authorization_commit: int
    examined: int
    returned: int
    complete: bool


class RecordsPage(Contract):
    items: tuple[RecordEntry, ...]
    next_cursor: str | None = None
    coverage: RecordsCoverage


class ReadRecords(Contract):
    references: tuple[CognitiveRef, ...] = Field(min_length=1, max_length=1000)
    purpose: RecordPurpose = "exact"


class RecordDetail(Contract):
    entry: RecordEntry
    history: RecordsPage
