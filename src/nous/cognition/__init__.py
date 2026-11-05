"""Formal cognitive records and namespace-bound application operations."""

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .retrieval.contracts import CognitiveQuery as CognitiveQuery
    from .writing.contracts import AfterWrite as AfterWrite
    from .writing.contracts import DraftOverride as DraftOverride
    from .writing.contracts import RecordOutcome as RecordOutcome
    from .writing.contracts import SourceChanges as SourceChanges
    from .writing.contracts import SourceRequest as SourceRequest
__all__ = [
    "AfterWrite", "CognitiveQuery", "DraftOverride", "RecordOutcome", "SourceChanges", "SourceRequest"
]


def __getattr__(name: str) -> object:
    if name == "CognitiveQuery":
        from .retrieval.contracts import CognitiveQuery

        return CognitiveQuery
    if name in {"AfterWrite", "DraftOverride", "RecordOutcome", "SourceChanges", "SourceRequest"}:
        from .writing import contracts

        return getattr(contracts, name)
    raise AttributeError(name)
