"""Wave native bindings; packaged configuration resources load independently."""

from importlib import import_module
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from ._core import (  # pyright: ignore[reportMissingModuleSource]
        Associations,
        CancellationToken,
        Candidates,
        Concepts,
        ContextResult,
        NativeEngine,
        NativeError,
        NativeQueryResult,
        NativeSnapshot,
        QueryObservation,
        SegmentResult,
        SourceFacts,
        aggregate_context,
        defaults_json,
        schemas_json,
        segment,
    )

__all__ = [
    "Associations",
    "CancellationToken",
    "Candidates",
    "Concepts",
    "ContextResult",
    "NativeEngine",
    "NativeError",
    "NativeQueryResult",
    "NativeSnapshot",
    "QueryObservation",
    "SegmentResult",
    "SourceFacts",
    "aggregate_context",
    "defaults_json",
    "schemas_json",
    "segment",
]


def __getattr__(name: str) -> object:
    if name not in __all__:
        raise AttributeError(name)
    value = getattr(import_module("._core", __name__), name)
    globals()[name] = value
    return value
