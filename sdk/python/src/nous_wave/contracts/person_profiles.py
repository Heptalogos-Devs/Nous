"""Source-backed controls and explicit single-person profile formation."""

from typing import Literal

from pydantic import Field

from .cognition import CognitiveRecord, CognitiveRef, EvidenceLink
from .common import CallContext, Contract, Operation


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


class PersonProfileRefreshIntent(Contract):
    context: CallContext
    command: RefreshPersonProfile
    attribute_indices: tuple[int, ...]
    claim_refs: tuple[CognitiveRef, ...] = ()


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
