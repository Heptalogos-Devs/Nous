"""Source-backed controls and explicit single-person profile formation."""

from typing import Literal

from pydantic import Field

from .cognition import CognitiveRecord, CognitiveRef, EpistemicStance, EvidenceLink
from .common import Contract, Operation, TimeRange


class PersonProfilesQuery(Contract):
    reference: CognitiveRef | None = None
    identifier: str = ""
    platform: str | None = None
    name: str = ""
    limit: int = Field(default=50, ge=1, le=1000)
    evidence_limit: int = Field(default=10, ge=0, le=1000)
    context_text: str = Field(default="", max_length=8192)


class PersonProfileEvidenceItem(Contract):
    key: str
    kind: Literal["entity_description", "entity_attribute", "fact_claim"]
    content: str
    reference: CognitiveRef
    evidence: tuple[EvidenceLink, ...]
    attribute_pointer: str | None = None
    epistemic: EpistemicStance | None = None
    validity: TimeRange | None = None
    status: str | None = None
    stability: str | None = None
    authority: str | None = None
    confidence: float | None = None


class PersonProfileView(Contract):
    record: CognitiveRecord
    evidence: tuple[PersonProfileEvidenceItem, ...]
    uncertain: tuple[PersonProfileEvidenceItem, ...]
    evidence_count: int
    uncertain_count: int
    watermark: int


class PersonProfilesResult(Contract):
    items: tuple[PersonProfileView, ...]


class PersonProfileChange(Operation):
    reference: CognitiveRef
    action: Literal[
        "set_aliases", "delete_aliases", "set_override", "delete_override", "correct_evidence"
    ]
    aliases: tuple[str, ...] | None = None
    text: str | None = None
    attribute_pointer: str | None = None
    evidence: EvidenceLink | None = None
    memory_reference: CognitiveRef | None = None
    reason: str = Field(default="Person profile management", min_length=1, max_length=4096)
    reported_by: str | None = None


class PersonProfileChangeResult(Contract):
    record: CognitiveRecord
    corrected_memory: CognitiveRecord | None = None
    report_material_id: str
    observation_id: str
    commit_seq: int


class RefreshPersonProfile(Operation):
    reference: CognitiveRef
    evidence_limit: int = Field(default=128, ge=1, le=256)


class PersonProfileRefreshSubmission(Contract):
    job_id: str
    reference: CognitiveRef


class PersonProfileEvidence(Contract):
    key: str
    claim: str
    epistemic: str
    validity: str


class PersonProfileModelInput(Contract):
    target: Literal["person"] = "person"
    name: str
    evidence: tuple[PersonProfileEvidence, ...]


class PersonProfileDraft(Contract):
    target: Literal["person"]
    summary: str
    aliases: tuple[str, ...]
    evidence_keys: tuple[str, ...] = Field(min_length=1)
