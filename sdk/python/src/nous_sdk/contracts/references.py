"""Readable secondary addresses retain canonical owner identities."""

from typing import Annotated, Literal

from pydantic import Field

from .cognition import CognitiveRef
from .cognitive_identity import CognitiveIdentity as CognitiveIdentity
from .common import Contract, ReferenceAddresses
from .context import CognitiveTarget, MediaTarget
from .media import MediaReference
from .resources import ResourceReference
from .source_objects import EvidenceReference, ObservationReference

ExactReference = (
    CognitiveRef | MediaReference | ResourceReference | ObservationReference | EvidenceReference
)


class ObjectTarget(Contract):
    kind: Literal["object"] = "object"
    identity: CognitiveIdentity


class ResourceTarget(Contract):
    kind: Literal["resource"] = "resource"
    reference: ResourceReference


class ObservationTarget(Contract):
    kind: Literal["observation"] = "observation"
    reference: ObservationReference


class EvidenceTarget(Contract):
    kind: Literal["evidence"] = "evidence"
    reference: EvidenceReference


ResolvedTarget = Annotated[
    CognitiveTarget | MediaTarget | ResourceTarget | ObservationTarget | EvidenceTarget,
    Field(discriminator="kind"),
]

SemanticTarget = Annotated[
    ObjectTarget
    | CognitiveTarget
    | MediaTarget
    | ResourceTarget
    | ObservationTarget
    | EvidenceTarget,
    Field(discriminator="kind"),
]


class ReferenceBinding(Contract):
    address: str
    target: SemanticTarget


class ReferenceResolution(Contract):
    namespace_id: str
    
    reference: ResolvedTarget
    addresses: ReferenceAddresses | None = None
