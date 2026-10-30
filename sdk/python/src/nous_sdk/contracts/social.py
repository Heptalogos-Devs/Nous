"""Social account correspondence and directional judgments retain their own evidence."""

from pydantic import Field

from .cognition import CognitiveRef, EvidenceLink, PlatformIdentity, SocialRelationBody
from .common import CognitiveTimes, Operation


class LinkIdentity(Operation):
    entity: CognitiveRef
    identity: PlatformIdentity
    times: CognitiveTimes
    replace: bool = False


class ChangeSocialRelation(Operation):
    body: SocialRelationBody
    evidence: tuple[EvidenceLink, ...] = Field(min_length=1)
    times: CognitiveTimes
    target: CognitiveRef | None = None
