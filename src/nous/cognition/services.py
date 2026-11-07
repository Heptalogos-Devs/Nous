"""Actual application services bound to one stable cognitive namespace."""

from dataclasses import dataclass

from .annotation.operations import AnnotationOperations
from .concepts import ConceptOperations
from .contracts import NamespaceRef
from .feedback import FeedbackOperations
from .retrieval.operations import CognitiveRetrieval
from .writing.generation import WritingOperations
from .writing.operations import RecordOperations


@dataclass(frozen=True)
class CognitiveServices:
    """Namespace-bound cognitive entry points assembled by the runtime.

    The services share the selected namespace and runtime-owned dependencies. Use
    records for formal revisions, writing for model-produced drafts, and retrieval
    for current or explicitly selected snapshots.
    """

    namespace: NamespaceRef
    records: RecordOperations
    writing: WritingOperations
    annotations: AnnotationOperations
    concepts: ConceptOperations
    retrieval: CognitiveRetrieval
    feedback: FeedbackOperations
