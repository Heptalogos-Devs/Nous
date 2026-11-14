"""Wave native bindings; packaged configuration resources load independently."""

from importlib import import_module
from importlib.metadata import PackageNotFoundError, distribution
from typing import TYPE_CHECKING

try:
    __path__ = [str(distribution("nous-server").locate_file("nous_native"))]
except PackageNotFoundError:
    __path__ = []

if TYPE_CHECKING:
    from ._core import (  # pyright: ignore[reportMissingModuleSource]
        Associations,
        CancellationToken,
        Candidates,
        CandidateSet,
        Concepts,
        ContextResult,
        NativeEngine,
        NativeError,
        NativeSnapshot,
        QueryObservation,
        SegmentResult,
        SourceFacts,
        aggregate_context,
        context_masses,
        defaults_json,
        schemas_json,
        segment,
    )

__all__ = [
    "Associations",
    "CancellationToken",
    "Candidates",
    "CandidateSet",
    "Concepts",
    "ContextResult",
    "NativeEngine",
    "NativeError",
    "NativeSnapshot",
    "QueryObservation",
    "SegmentResult",
    "SourceFacts",
    "aggregate_context",
    "context_masses",
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
