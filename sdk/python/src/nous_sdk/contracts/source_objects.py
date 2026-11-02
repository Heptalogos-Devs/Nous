"""Source events and evidence edges keep their own query identities."""

from typing import Literal

from pydantic import Field

from .cognition import CognitiveRef, EvidenceLink
from .common import CognitiveTimes, Contract, ReferenceAddresses
from .media import MediaReference


class ObservationReference(Contract):
    kind: Literal["observation"] = "observation"
    id: str
    revision: Literal[1] = 1

    def exact(self) -> str:
        return f"observation:{self.id}@1"


class EvidenceReference(Contract):
    kind: Literal["evidence"] = "evidence"
    id: str
    revision: Literal[1] = 1

    def exact(self) -> str:
        return f"evidence:{self.id}"


class ObservationRecord(Contract):
    reference: ObservationReference
    namespace_id: str
    
    source: MediaReference
    event_kind: str
    external_event_id: str | None = None
    caller_id: str | None = None
    participants: tuple[str, ...] = ()
    audience: tuple[str, ...] = ()
    text: str
    source_roots: tuple[str, ...]
    support_roots: tuple[str, ...] = ()
    source_regions: dict[str, tuple[str, ...]] = Field(default_factory=dict)
    times: CognitiveTimes
    commit_seq: int
    addresses: ReferenceAddresses | None = None


class EvidenceRelationRecord(Contract):
    reference: EvidenceReference
    namespace_id: str
    
    target: CognitiveRef
    evidence: EvidenceLink
    source: MediaReference | None = None
    text: str
    source_roots: tuple[str, ...]
    support_roots: tuple[str, ...] = ()
    source_regions: dict[str, tuple[str, ...]] = Field(default_factory=dict)
    times: CognitiveTimes
    commit_seq: int
    addresses: ReferenceAddresses | None = None
