"""Cognitive revisions retain their domain bodies and exact provenance."""

from __future__ import annotations

from typing import Annotated, Literal

from pydantic import Field, JsonValue, model_validator

from .common import CognitiveTimes, Contract, Operation, ReferenceAddresses, TimePoint, TimeRange
from .media import MediaReference

CognitiveKind = Literal[
    "memory",
    "memory_semantics",
    "concept",
    "entity",
    "episode",
    "journal",
    "schema",
    "association",
    "self_facet",
    "narrative_thread",
    "domain_knowledge",
    "social_relation",
    "language_convention",
    "desire",
    "concern",
]


class CognitiveRef(Contract):
    kind: CognitiveKind
    id: str
    revision: int = Field(ge=1)

    def exact(self) -> str:
        return f"{self.kind}:{self.id}@{self.revision}"


class EpistemicStance(Contract):
    classification: Literal[
        "observed", "reported", "derived", "inferred", "narrative", "simulated"
    ] = "reported"
    attribution: str | None = None
    confidence: float | None = Field(default=None, ge=0, le=1)


class EvidenceLink(Contract):
    observation_id: str | None = None
    reference: CognitiveRef | None = None
    region_id: str | None = None
    polarity: Literal["support", "counterexample", "boundary"] = "support"
    role: str = "grounding"

    @model_validator(mode="after")
    def source_identity(self) -> EvidenceLink:
        if (self.observation_id is None) == (self.reference is None):
            raise ValueError("Evidence requires exactly one observation or cognitive revision")
        if self.region_id is not None and self.observation_id is None:
            raise ValueError("A source region requires its actual observation")
        return self


class MemoryBody(Contract):
    kind: Literal["memory"] = "memory"
    text: str = Field(min_length=1)
    summary: str | None = None
    tags: tuple[str, ...] = ()
    external_ids: dict[str, str] = Field(default_factory=dict)
    metadata: dict[str, JsonValue] = Field(default_factory=dict)
    structured: JsonValue = None
    role: Literal["experiential", "declarative", "procedural_experience"] = "declarative"
    formation_mode: Literal["grounded", "synthesized"] = "grounded"
    epistemic: EpistemicStance = Field(default_factory=EpistemicStance)


class MemoryInterpretation(Contract):
    text: str = Field(min_length=1)
    role: Literal["experiential", "declarative", "procedural_experience"]
    formation_mode: Literal["grounded", "synthesized"]
    epistemic: EpistemicStance
    tags: tuple[str, ...] = ()


class MemorySemanticsBody(Contract):
    kind: Literal["memory_semantics"] = "memory_semantics"
    memory: CognitiveRef
    cognitive_role: str
    formation_mode: Literal["grounded", "synthesized"]
    epistemic: EpistemicStance
    concepts: tuple[CognitiveRef, ...] = ()
    qualifications: tuple[str, ...] = ()
    interpretations: tuple[MemoryInterpretation, ...] = ()
    grounding_observation: str | None = None
    event_expression: str | None = None
    event_parse_status: Literal["not_provided", "resolved", "unresolved"] = "not_provided"


class ConceptBody(Contract):
    kind: Literal["concept"] = "concept"
    label: str = Field(min_length=1)
    description: str = ""
    aliases: tuple[str, ...] = ()
    canonical: CognitiveRef | None = None
    members: tuple[CognitiveRef | MediaReference, ...] = ()
    lineage: tuple[CognitiveRef, ...] = ()
    state: Literal["active", "merged", "split"] = "active"
    successors: tuple[CognitiveRef, ...] = ()


class EntityAttribute(Contract):
    predicate: str = Field(min_length=1)
    value: str
    epistemic: EpistemicStance
    validity: TimeRange | None = None
    evidence: tuple[EvidenceLink, ...] = Field(min_length=1)

    @model_validator(mode="after")
    def positive_support(self) -> EntityAttribute:
        if not any(link.polarity == "support" for link in self.evidence):
            raise ValueError("Entity attributes require their own positive evidence")
        return self


class EntityMention(Contract):
    reference: CognitiveRef | MediaReference
    role: str = "mentioned"


class EntityBody(Contract):
    kind: Literal["entity"] = "entity"
    label: str
    entity_type: Literal["person", "group", "organization", "place", "object", "other"] = "person"
    aliases: tuple[str, ...] = ()
    description: str = ""
    identifiers: dict[str, str] = Field(default_factory=dict)
    attributes: tuple[EntityAttribute, ...] = ()
    mentions: tuple[EntityMention, ...] = ()


class EpisodeMember(Contract):
    reference: CognitiveRef
    role: str = "event"
    ordinal: int = Field(ge=0)


class EpisodeBody(Contract):
    kind: Literal["episode"] = "episode"
    title: str
    track: str
    experience: TimeRange
    parent: CognitiveRef | None = None
    members: tuple[EpisodeMember, ...]
    boundary_explanation: str
    lineage: tuple[CognitiveRef, ...] = ()
    state: Literal["active", "resegmented"] = "active"
    successors: tuple[CognitiveRef, ...] = ()


class JournalBody(Contract):
    kind: Literal["journal"] = "journal"
    title: str
    narrative: str
    period: TimeRange
    knowledge_cutoff: int = Field(ge=0)
    perspective: EpistemicStance
    episodes: tuple[CognitiveRef, ...] = ()
    lineage: tuple[CognitiveRef, ...] = ()


class SchemaBody(Contract):
    kind: Literal["schema"] = "schema"
    title: str
    claim: str
    scope: str
    boundary: str
    support: tuple[CognitiveRef, ...]
    counterexamples: tuple[CognitiveRef, ...] = ()
    lineage: tuple[CognitiveRef, ...] = ()
    epistemic: EpistemicStance = Field(default_factory=EpistemicStance)


class AssociationBody(Contract):
    kind: Literal["association"] = "association"
    from_ref: CognitiveRef
    to_ref: CognitiveRef
    relation: str
    polarity: Literal["positive", "negative"] = "positive"
    basis_class: Literal[
        "host_explicit",
        "source_evidence",
        "cognitive_derivation",
        "meaningful_use",
        "derived_structure",
    ]
    description: str = ""


class SelfFacetBody(Contract):
    kind: Literal["self_facet"] = "self_facet"
    facet: str
    description: str
    epistemic: EpistemicStance = Field(default_factory=EpistemicStance)
    contexts: tuple[str, ...] = ()
    exceptions: tuple[str, ...] = ()


class NarrativeThreadBody(Contract):
    kind: Literal["narrative_thread"] = "narrative_thread"
    title: str
    narrative: str
    experiences: tuple[CognitiveRef, ...]


class DomainKnowledgeBody(Contract):
    kind: Literal["domain_knowledge"] = "domain_knowledge"
    domain: str
    understanding: str
    experience: tuple[CognitiveRef, ...] = ()
    exceptions: tuple[str, ...] = ()
    uncertainty: str = ""


class SocialRelationBody(Contract):
    kind: Literal["social_relation"] = "social_relation"
    from_entity: CognitiveRef
    to_entity: CognitiveRef
    relation: str
    degree: float = Field(ge=-1, le=1)
    trust_domains: dict[str, float] = Field(default_factory=dict)
    scope: tuple[str, ...] = ()
    judgement: Literal["sufficient", "insufficient", "unknown"] = "unknown"


class LanguageConventionBody(Contract):
    kind: Literal["language_convention"] = "language_convention"
    expression: str
    meaning: str
    audience: tuple[CognitiveRef, ...]
    pragmatics: str
    contexts: tuple[str, ...] = ()


class DesiredCondition(Contract):
    operator: Literal["all", "any", "not", "predicate"] = "predicate"
    predicate: str | None = None
    children: tuple[DesiredCondition, ...] = ()
    mode: Literal["achieve", "cease", "maintain", "avoid"] = "achieve"
    hold: TimeRange | None = None
    state: Literal["true", "false", "unknown"] = "unknown"
    evidence: tuple[CognitiveRef, ...] = ()

    @model_validator(mode="after")
    def meaningful_condition(self) -> DesiredCondition:
        if self.operator == "predicate" and (not self.predicate or self.children):
            raise ValueError("A predicate requires its expression and no child conditions")
        if self.operator in {"all", "any"} and (not self.children or self.predicate):
            raise ValueError("A composite condition requires children")
        if self.operator == "not" and (len(self.children) != 1 or self.predicate):
            raise ValueError("A negated condition requires exactly one child")
        if self.state != "unknown" and self.operator == "predicate" and not self.evidence:
            raise ValueError("An assessed predicate requires actual cognitive evidence")
        return self


class DesireBody(Contract):
    kind: Literal["desire"] = "desire"
    description: str
    condition: DesiredCondition
    strength: float = Field(ge=0, le=1)


class ConcernBody(Contract):
    kind: Literal["concern"] = "concern"
    description: str
    condition: DesiredCondition
    importance: float = Field(ge=0, le=1)


CognitiveBody = Annotated[
    MemoryBody
    | MemorySemanticsBody
    | ConceptBody
    | EntityBody
    | EpisodeBody
    | JournalBody
    | SchemaBody
    | AssociationBody
    | SelfFacetBody
    | NarrativeThreadBody
    | DomainKnowledgeBody
    | SocialRelationBody
    | LanguageConventionBody
    | DesireBody
    | ConcernBody,
    Field(discriminator="kind"),
]


class CognitiveRecord(Contract):
    reference: CognitiveRef
    addresses: ReferenceAddresses | None = None
    namespace_id: str
    subject_id: str
    body: CognitiveBody | None
    evidence: tuple[EvidenceLink, ...]
    source_roots: tuple[str, ...]
    support_roots: tuple[str, ...] = ()
    source_regions: dict[str, tuple[str, ...]] = Field(default_factory=dict)
    times: CognitiveTimes
    producer_call_id: str | None = None
    commit_seq: int
    deleted: bool = False
    purged: bool = False
    lifecycle: MemoryLifecycle | None = None
    cause: RevisionCause | None = None

    @model_validator(mode="after")
    def tombstone_body(self) -> CognitiveRecord:
        if self.body is None and not self.purged:
            raise ValueError("Only a purged cognitive tombstone may omit its body")
        if self.purged and self.body is not None:
            raise ValueError("Purged cognitive records cannot retain their body")
        return self


class AdoptCognition(Operation):
    body: CognitiveBody
    evidence: tuple[EvidenceLink, ...]
    times: CognitiveTimes | None = None
    id: str | None = None
    expected_revision: int = Field(default=0, ge=0)
    producer_call_id: str | None = None


class ProposedEntityAttribute(Contract):
    predicate: str
    value: str
    epistemic: EpistemicStance = Field(default_factory=EpistemicStance)
    validity: TimeRange | None = None


class ProposedEntity(Contract):
    key: str = Field(
        min_length=1,
        description="Unique local key declared by this proposal. Do not use the reserved key memory or shadow a source.formation_catalog key.",
    )
    label: str = Field(min_length=1)
    entity_type: Literal["person", "group", "organization", "place", "object", "other"] = "other"
    aliases: tuple[str, ...] = ()
    description: str = ""
    identifiers: dict[str, str] = Field(default_factory=dict)
    attributes: tuple[ProposedEntityAttribute, ...] = ()
    target_key: str | None = Field(
        default=None,
        description="For revising an existing identity, use its exact source.formation_catalog key; otherwise leave null.",
    )


class ProposedConcept(Contract):
    key: str = Field(
        min_length=1,
        description="Unique local key declared by this proposal. Do not use the reserved key memory or shadow a source.formation_catalog key.",
    )
    label: str = Field(min_length=1)
    aliases: tuple[str, ...] = ()
    description: str = ""
    member_keys: tuple[str, ...] = Field(
        default=("memory",),
        description="Members are reserved memory, declared proposal keys, or supplied source.formation_catalog keys.",
    )
    target_key: str | None = Field(
        default=None,
        description="For revising an existing identity, use its exact source.formation_catalog key; otherwise leave null.",
    )


class ProposedSchema(Contract):
    key: str = Field(
        min_length=1,
        description="Unique local key declared by this proposal. Do not use the reserved key memory or shadow a source.formation_catalog key.",
    )
    title: str
    claim: str
    scope: str
    boundary: str
    support_keys: tuple[str, ...] = ("memory",)
    counterexample_keys: tuple[str, ...] = ()
    epistemic: EpistemicStance = Field(default_factory=EpistemicStance)
    target_key: str | None = Field(
        default=None,
        description="For revising an existing identity, use its exact source.formation_catalog key; otherwise leave null.",
    )


class FormationCatalogEntry(Contract):
    key: str
    reference: CognitiveRef
    label: str
    aliases: tuple[str, ...] = ()
    description: str = ""


class ProposedAssociation(Contract):
    from_key: str = Field(
        description="Exactly memory, a key declared in entities/concepts/schemas, or a source.formation_catalog key; never a free-form label or invented ID."
    )
    to_key: str = Field(
        description="Exactly memory, a key declared in entities/concepts/schemas, or a source.formation_catalog key; never a free-form label or invented ID."
    )
    relation: str
    polarity: Literal["positive", "negative"] = "positive"
    description: str = ""


class FormationDraft(Contract):
    memories: tuple[MemoryBody, ...]
    event_expression: str | None = None
    entities: tuple[ProposedEntity, ...] = ()
    associations: tuple[ProposedAssociation, ...] = ()
    concepts: tuple[ProposedConcept, ...] = ()
    schemas: tuple[ProposedSchema, ...] = ()


class FormationProposal(FormationDraft):
    producer_call_id: str | None = None
    event: TimePoint | None = None
    validity: TimeRange | None = None


class EmbeddingBatch(Contract):
    model: str
    space: str
    dimensions: int = Field(gt=0)
    vectors: tuple[tuple[float, ...], ...]
    call_id: str | None = None
    call_ids: tuple[str, ...] = ()


class MergeConcepts(Operation):
    target: CognitiveRef
    sources: tuple[CognitiveRef, ...] = Field(min_length=1)
    reason: str = Field(min_length=1)


class ConceptPartition(Contract):
    label: str = Field(min_length=1)
    members: tuple[CognitiveRef, ...] = Field(min_length=1)


class SplitConcept(Operation):
    source: CognitiveRef
    partitions: tuple[ConceptPartition, ...] = Field(min_length=2)
    reason: str = Field(min_length=1)


class ResegmentEpisodes(Operation):
    originals: tuple[CognitiveRef, ...] = Field(min_length=1)
    replacements: tuple[EpisodeBody, ...] = Field(min_length=1)
    reason: str = Field(min_length=1)


class MemoryLifecycle(Contract):
    strength: float = Field(default=1, ge=0, le=1)
    anchor_unix_ms: int
    activation: Literal["active", "frozen", "forgotten", "weakened", "decayed"] = "active"
    pinned: bool = False
    protected_until_unix_ms: int | None = None
    inactive_since_unix_ms: int | None = None
    last_reinforced_unix_ms: int | None = None

    def protected(self, now_unix_ms: int) -> bool:
        return (
            self.pinned
            or self.protected_until_unix_ms is not None
            and self.protected_until_unix_ms > now_unix_ms
        )


class RevisionCause(Contract):
    action: str
    actor: str
    reason: str
    parent: CognitiveRef | None = None


CognitiveRecord.model_rebuild()
