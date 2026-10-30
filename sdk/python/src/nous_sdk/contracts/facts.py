"""Public fact operations produce exact source-backed claim revisions."""

from typing import Annotated, Literal

from pydantic import Field

from .cognition import CognitiveRecord, CognitiveRef
from .cognitive_identity import CognitiveIdentity
from .common import Contract, Operation, TimePoint, TimeRange
from .fact_body import (
    FactAuthority,
    FactClaimValue,
    FactEvidence,
    FactScope,
    FactSection,
    FactStability,
    FactStatus,
)


class PersonFactScopeInput(Contract):
    kind: Literal["person"] = "person"
    person: CognitiveRef | CognitiveIdentity


class ChatFactScopeInput(Contract):
    kind: Literal["chat"] = "chat"
    conversation_ref: str = Field(min_length=1)


FactScopeInput = Annotated[PersonFactScopeInput | ChatFactScopeInput, Field(discriminator="kind")]


class FactClaimInput(FactClaimValue):
    scope: FactScopeInput


class FactClaimPatch(Contract):
    fact_key: str | None = None
    value_text: str | None = None
    polarity: Literal["positive", "negative"] | None = None
    cardinality: Literal["single", "set"] | None = None
    stability: FactStability | None = None
    profile_section: FactSection | None = None
    authority: FactAuthority | None = None
    confidence: float | None = Field(default=None, ge=0, le=1)


class FactChange(Operation):
    action: Literal["assert", "update", "classify", "retract", "restore", "add_evidence"]
    reference: CognitiveRef | None = None
    claim_id: str | None = None
    claim: FactClaimInput | None = None
    patch: FactClaimPatch | None = None
    expected_scope: FactScopeInput | None = None
    expected_scope_kind: Literal["person", "chat"] | None = None
    evidence: tuple[FactEvidence, ...] = ()
    supersedes: tuple[CognitiveRef, ...] = ()
    validity: TimeRange | None = None
    validity_patch: TimeRange | None = None
    reason: str = Field(default="Explicit fact management", min_length=1)
    reported_by: str | None = None
    refresh_profile: bool = True


class FactChangeResult(Contract):
    record: CognitiveRecord
    changed: tuple[CognitiveRecord, ...] = ()
    conflicting: tuple[CognitiveRef, ...] = ()
    report_material_id: str
    observation_id: str
    commit_seq: int
    profile_job_id: str | None = None


class FactQuery(Contract):
    scope: FactScope | None = None
    statuses: tuple[FactStatus, ...] = ("active",)
    effective_at: TimePoint | None = None
    query: str = ""
    limit: int = Field(default=200, ge=1, le=1000)


class FactView(Contract):
    successors: tuple[CognitiveRef, ...] = ()
    record: CognitiveRecord
    history: tuple[CognitiveRecord, ...]
    conflicting: tuple[CognitiveRef, ...] = ()


class FactRead(Contract):
    reference: CognitiveRef | str
