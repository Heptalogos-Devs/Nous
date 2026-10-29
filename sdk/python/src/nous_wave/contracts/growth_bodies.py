"""Model-owned semantic proposals contain local keys and no authority identities."""

from __future__ import annotations

from typing import Annotated, Literal

from pydantic import Field

from .cognition import EpistemicStance, MemoryInterpretation
from .common import Contract, TimeRange


class GrowthRef(Contract):
    key: str = Field(
        min_length=1, description="An exact supplied catalog key or an earlier proposal key"
    )


class GrowthMemory(Contract):
    kind: Literal["memory"]
    text: str = Field(min_length=1)
    summary: str | None = None
    tags: tuple[str, ...] = ()
    role: Literal["experiential", "declarative", "procedural_experience"] = "declarative"
    formation_mode: Literal["grounded", "synthesized"] = "synthesized"
    epistemic: EpistemicStance


class GrowthMemorySemantics(Contract):
    kind: Literal["memory_semantics"]
    memory: GrowthRef
    cognitive_role: str
    formation_mode: Literal["grounded", "synthesized"]
    epistemic: EpistemicStance
    concepts: tuple[GrowthRef, ...] = ()
    qualifications: tuple[str, ...] = ()
    interpretations: tuple[MemoryInterpretation, ...] = ()
    event_expression: str | None = None


class GrowthConcept(Contract):
    kind: Literal["concept"]
    label: str = Field(min_length=1)
    aliases: tuple[str, ...] = ()
    description: str = ""
    members: tuple[GrowthRef, ...] = ()


class GrowthAttribute(Contract):
    predicate: str = Field(min_length=1)
    value: str
    epistemic: EpistemicStance
    validity: TimeRange | None = None
    evidence: tuple[GrowthRef, ...] = Field(min_length=1)


class GrowthMention(Contract):
    reference: GrowthRef
    role: str = "mentioned"


class GrowthEntity(Contract):
    kind: Literal["entity"]
    label: str
    entity_type: Literal["person", "group", "organization", "place", "object", "other"]
    aliases: tuple[str, ...] = ()
    description: str = ""
    attributes: tuple[GrowthAttribute, ...] = ()
    mentions: tuple[GrowthMention, ...] = ()


class GrowthEpisodeMember(Contract):
    reference: GrowthRef
    role: str = "event"
    ordinal: int = Field(ge=0)


class GrowthEpisode(Contract):
    kind: Literal["episode"]
    title: str
    track: str
    experience: TimeRange
    parent: GrowthRef | None = None
    members: tuple[GrowthEpisodeMember, ...]
    boundary_explanation: str


class GrowthJournal(Contract):
    kind: Literal["journal"]
    title: str
    narrative: str
    period: TimeRange
    perspective: EpistemicStance
    episodes: tuple[GrowthRef, ...] = ()


class GrowthSchema(Contract):
    kind: Literal["schema"]
    title: str
    claim: str
    scope: str
    boundary: str
    support: tuple[GrowthRef, ...] = Field(min_length=1)
    counterexamples: tuple[GrowthRef, ...] = ()
    epistemic: EpistemicStance


class GrowthAssociation(Contract):
    kind: Literal["association"]
    from_ref: GrowthRef
    to_ref: GrowthRef
    relation: str
    polarity: Literal["positive", "negative"] = "positive"
    description: str = ""


class GrowthSelfFacet(Contract):
    kind: Literal["self_facet"]
    facet: str
    description: str
    epistemic: EpistemicStance
    contexts: tuple[str, ...] = ()
    exceptions: tuple[str, ...] = ()


class GrowthNarrativeThread(Contract):
    kind: Literal["narrative_thread"]
    title: str
    narrative: str
    experiences: tuple[GrowthRef, ...]


class GrowthDomainKnowledge(Contract):
    kind: Literal["domain_knowledge"]
    domain: str
    understanding: str
    experience: tuple[GrowthRef, ...] = ()
    exceptions: tuple[str, ...] = ()
    uncertainty: str = ""


class GrowthSocialRelation(Contract):
    kind: Literal["social_relation"]
    from_entity: GrowthRef
    to_entity: GrowthRef
    relation: str
    dimension: Literal[
        "familiarity",
        "affinity",
        "intimacy",
        "trust",
        "cooperation",
        "guidance",
        "membership",
        "conflict",
        "other",
    ]
    degree: float = Field(ge=-1, le=1, allow_inf_nan=False)
    trust_domains: dict[str, float] = Field(default_factory=dict)
    scope: tuple[str, ...] = ()
    groups: tuple[GrowthRef, ...] = ()
    epistemic: EpistemicStance
    judgement: Literal["sufficient", "insufficient", "unknown"] = "unknown"


class GrowthLanguageConvention(Contract):
    kind: Literal["language_convention"]
    expression: str
    meaning: str
    audience: tuple[GrowthRef, ...] = Field(min_length=1)
    pragmatics: str
    contexts: tuple[str, ...] = ()


class GrowthCondition(Contract):
    operator: Literal["all", "any", "not", "predicate"]
    predicate: str | None = None
    children: tuple[GrowthCondition, ...] = ()
    mode: Literal["achieve", "cease", "maintain", "avoid"]
    hold: TimeRange | None = None
    state: Literal["true", "false", "unknown"] = "unknown"
    evidence: tuple[GrowthRef, ...] = ()


class GrowthDesire(Contract):
    kind: Literal["desire"]
    description: str
    condition: GrowthCondition
    strength: float = Field(ge=0, le=1)


class GrowthConcern(Contract):
    kind: Literal["concern"]
    description: str
    condition: GrowthCondition
    importance: float = Field(ge=0, le=1)


GrowthBody = Annotated[
    GrowthMemory
    | GrowthMemorySemantics
    | GrowthConcept
    | GrowthEntity
    | GrowthEpisode
    | GrowthJournal
    | GrowthSchema
    | GrowthAssociation
    | GrowthSelfFacet
    | GrowthNarrativeThread
    | GrowthDomainKnowledge
    | GrowthSocialRelation
    | GrowthLanguageConvention
    | GrowthDesire
    | GrowthConcern,
    Field(discriminator="kind"),
]
