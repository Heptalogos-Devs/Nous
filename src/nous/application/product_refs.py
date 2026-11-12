"""Formal references that can be reported after a successful adoption."""

from typing import Annotated

from pydantic import Field

from nous.cognition.concept_contracts import AnnotationRef, ConceptRevisionRef
from nous.cognition.contracts import FeedbackRef, NamespaceRef, RecordRevisionRef
from nous.cognition.writing.contracts import DraftRef
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
    | SnapshotRef,
    Field(discriminator="kind"),
]
