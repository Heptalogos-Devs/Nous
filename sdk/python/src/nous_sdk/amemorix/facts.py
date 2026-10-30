"""The actual fact-admin arguments preserve omitted update fields and explicit time bounds."""

from pydantic import Field

from ..contracts.common import Contract
from ..contracts.fact_body import FactAuthority, FactSection, FactStability, FactStatus


class FactAdminInput(Contract):
    action: str
    operation_id: str | None = None
    claim_id: str = ""
    scope_type: str = "person"
    scope_id: str = ""
    fact_key: str | None = None
    value_text: str | None = None
    polarity: str | None = None
    cardinality: str | None = None
    stability: FactStability | None = None
    profile_section: FactSection | None = None
    authority: FactAuthority | None = None
    confidence: float | None = Field(default=None, ge=0, le=1)
    valid_from: float | None = None
    valid_to: float | None = None
    statuses: tuple[FactStatus, ...] | None = None
    limit: int = Field(default=200, ge=1, le=1000)
    reason: str = "Explicit fact administration"
    updated_by: str | None = None
    requested_by: str | None = None
    refresh_profile: bool = True
