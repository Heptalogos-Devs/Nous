"""Readable secondary addresses retain canonical owner identities."""

from typing import Annotated, Literal

from pydantic import Field

from .cognition import CognitiveKind
from .common import Contract, ReferenceAddresses
from .context import CognitiveTarget, ContextTarget, MediaTarget


class CognitiveIdentity(Contract):
    kind: CognitiveKind
    id: str


class ObjectTarget(Contract):
    kind: Literal["object"] = "object"
    identity: CognitiveIdentity


SemanticTarget = Annotated[
    ObjectTarget | CognitiveTarget | MediaTarget, Field(discriminator="kind")
]


class ReferenceBinding(Contract):
    address: str
    target: SemanticTarget


class ReferenceResolution(Contract):
    namespace_id: str
    subject_id: str
    reference: ContextTarget
    addresses: ReferenceAddresses | None = None
