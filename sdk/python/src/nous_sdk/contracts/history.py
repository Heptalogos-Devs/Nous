"""Program-selected complete native history ranges and their continuation summaries."""
from pydantic import Field
from .common import Contract
from .targets import ContextTarget

class WorkingHistoryGroup(Contract):
    group_id: str
    label: str | None = None
    sources: tuple[ContextTarget,...]
    purpose: str
    continuation: str
    anchors: tuple[str,...] = ()
    token_budget: int = Field(ge=1)
    start: int = Field(ge=0)
    end: int = Field(gt=0)

class HistorySummary(Contract):
    group_id: str
    text: str
    covered_sources: tuple[ContextTarget,...]
    producer_call_id: str | None = None
