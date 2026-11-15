"""Formal references that can be reported after a successful adoption."""

from typing import Annotated

from pydantic import Field

from nous.cognition.concept_contracts import AnnotationRef, ConceptRevisionRef
from nous.cognition.contracts import FeedbackRef, NamespaceRef
from nous.cognition.writing.contracts import DraftRef
from nous.common.material_refs import RecordRevisionRef, WorkRef, WorkRevisionRef
from nous.content.contracts import ArtifactRef, ResultRef
from nous.wave.contracts import SnapshotRef

type ProductRef = Annotated[
    ArtifactRef
    | ResultRef
    | NamespaceRef
    | RecordRevisionRef
    | ConceptRevisionRef
    | AnnotationRef
    | FeedbackRef
    | DraftRef
    | WorkRef
    | WorkRevisionRef
    | SnapshotRef,
    Field(discriminator="kind"),
]
