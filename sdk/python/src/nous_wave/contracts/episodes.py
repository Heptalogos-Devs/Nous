"""Source-backed Episode directories and durable complete-source segmentation."""

from pydantic import Field

from .cognition import CognitiveRecord, CognitiveRef
from .common import CallContext, Contract, Operation, TimeRange


class EpisodeQuery(Contract):
    query: str = ""
    source: str | None = None
    person: CognitiveRef | None = None
    experience: TimeRange | None = None
    limit: int = Field(default=20, ge=1, le=1000)


class EpisodeView(Contract):
    record: CognitiveRecord
    members: tuple[CognitiveRecord, ...]
    sources: tuple[str, ...]


class EpisodeGet(Contract):
    reference: CognitiveRef
    paragraph_limit: int = Field(default=100, ge=1, le=1000)


class ProcessEpisodeSources(Contract):
    limit: int = Field(default=20, ge=1, le=1000)
    max_retry: int = Field(default=3, ge=0)


class RebuildEpisodes(Operation):
    sources: tuple[str, ...] = Field(min_length=1, max_length=1000)
    group_gap_ms: int = Field(default=86_400_000, ge=0)
    max_members_per_call: int = Field(default=20, ge=1, le=1000)
    max_characters_per_call: int = Field(default=6000, ge=200)


class EpisodeRebuildSubmission(Contract):
    job_id: str
    sources: tuple[str, ...]


class EpisodeSourceSnapshot(Contract):
    source: str
    members: tuple[CognitiveRef, ...]
    originals: tuple[CognitiveRef, ...]
    observations: tuple[str, ...]


class EpisodeRebuildIntent(Contract):
    context: CallContext
    command: RebuildEpisodes
    snapshots: tuple[EpisodeSourceSnapshot, ...]


class EpisodeSegmentationMember(Contract):
    key: str
    reference: CognitiveRef
    content: str
    event: TimeRange | None = None


class EpisodeSegmentationInput(Contract):
    source: str
    members: tuple[EpisodeSegmentationMember, ...] = Field(min_length=1)


class EpisodeSegment(Contract):
    title: str = Field(min_length=1)
    summary: str = Field(min_length=1)
    member_keys: tuple[str, ...] = Field(min_length=1)
    experience: TimeRange
    boundary_explanation: str = Field(min_length=1)


class EpisodeSegmentationDraft(Contract):
    episodes: tuple[EpisodeSegment, ...] = Field(min_length=1)
