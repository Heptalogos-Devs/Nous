from collections.abc import Sequence

import numpy as np
from numpy import dtype, ndarray

# 解决 NDArray 类型推导问题
type Array[T: np.generic] = ndarray[tuple[int, ...], dtype[T]]

class NativeError(RuntimeError):
    code: str
    details_json: str

class CancellationToken:
    """Request cooperative cancellation of Native work using this token.

    Cancellation is permanent for this token. Calling ``cancel`` requests
    termination; wait for the associated work to return before releasing its
    resources.
    """
    def __init__(self) -> None: ...
    def cancel(self) -> None: ...
    @property
    def cancelled(self) -> bool: ...

class Candidates:
    # metadata: {keys: [str], documents: [str], group_keys: [str], group_rows: [int]} (one curve document per row)
    def __init__(
        self,
        metadata_json: str,
        texts: Sequence[str],
        vectors: Array[np.float32] | None,
        membership_indptr: Array[np.uint64],
        membership_indices: Array[np.uint64],
        path_indptr: Array[np.uint64],
        path_step_indptr: Array[np.uint64],
        path_indices: Array[np.uint64],
        path_positions: Array[np.int64],
        root_indptr: Array[np.uint64],
        root_indices: Array[np.uint64],
    ) -> None: ...

class Concepts:
    # metadata: {keys: [str], labels: [str], aliases: [[str]]}
    def __init__(self, metadata_json: str, vectors: Array[np.float32] | None) -> None: ...

class SourceFacts:
    # metadata: {root_keys: [str], root_checksums: [str], sequence_keys: [str]}
    def __init__(
        self,
        metadata_json: str,
        sequence_roots: Array[np.uint64],
        indptr: Array[np.uint64],
        indices: Array[np.uint64],
        positions: Array[np.int64],
        ordered: Array[np.bool_],
    ) -> None: ...

class Associations:
    # metadata: {keys: [str], relations: [str]}
    def __init__(
        self,
        metadata_json: str,
        sources: Array[np.uint64],
        targets: Array[np.uint64],
        directed: Array[np.bool_],
        strengths: Array[np.float64],
        confidences: Array[np.float64],
        polarities: Array[np.int8],
        edge_groups: Array[np.uint64],
        group_root_indptr: Array[np.uint64],
        group_root_indices: Array[np.uint64],
    ) -> None: ...

class CandidateSet:
    def __init__(self, value_json: str) -> None: ...
    @property
    def value_json(self) -> str: ...

class QueryObservation:
    @property
    def diagnostics_json(self) -> str: ...

class NativeQueryResult:
    @property
    def row_ids(self) -> Array[np.uint64]: ...
    @property
    def quality_scores(self) -> Array[np.float64]: ...
    @property
    def ranking_scores(self) -> Array[np.float64]: ...
    @property
    def result_json(self) -> str: ...

class NativeSnapshot:
    """Immutable loaded snapshot whose calculations use its engine's worker pool.

    Observations and candidate pools are tied to the snapshot that produced
    them. Keep the engine open while using the snapshot and pass a cancellation
    token to long-running operations when cooperative cancellation is needed.
    """
    @property
    def identity_json(self) -> str: ...
    @property
    def capabilities(self) -> list[str]: ...
    @property
    def statistics_json(self) -> str: ...
    def observe(
        self,
        vector: Array[np.float32],
        *,
        text: str = "",
        options_json: str = "{}",
        ghost_vectors: Array[np.float32] | None = None,
        cancel: CancellationToken | None = None,
    ) -> QueryObservation: ...
    def execute_program(
        self,
        *,
        name: str,
        program_json: str,
        text: str = "",
        raw_vector: Array[np.float32] | None = None,
        contextual_vector: Array[np.float32] | None = None,
        allowed_rows: Array[np.uint64] | None = None,
        options_json: str = "{}",
        ghost_vectors: Array[np.float32] | None = None,
        candidate_set: CandidateSet | None = None,
        cancel: CancellationToken | None = None,
    ) -> NativeQueryResult: ...
    def execute_expression(
        self,
        *,
        name: str,
        program_json: str,
        expression_json: str,
        text: str = "",
        original_vector: Array[np.float32] | None = None,
        raw_vectors: Array[np.float32] | None = None,
        contextual_vectors: Array[np.float32] | None = None,
        allowed_rows: Array[np.uint64] | None = None,
        options_json: str = "{}",
        ghost_vectors: Array[np.float32] | None = None,
        cancel: CancellationToken | None = None,
    ) -> NativeQueryResult: ...
    def think(
        self,
        observation: QueryObservation,
        stages_json: str,
        *,
        options_json: str = "{}",
        cancel: CancellationToken | None = None,
    ) -> str: ...
    def neighborhood(
        self,
        rows: Array[np.uint64],
        *,
        options_json: str = "{}",
        cancel: CancellationToken | None = None,
    ) -> str: ...

class NativeEngine:
    """Own a bounded Native worker pool for snapshot build, open and queries.

    ``close`` stops new work and waits for active work to leave the pool. Loaded
    snapshots share the engine and cannot run calculations after it is closed.
    The runtime owns and closes its engine after its operations have finished.
    """
    def __init__(self, settings_json: str = "{}") -> None: ...
    def build(
        self,
        candidates: Candidates,
        concepts: Concepts,
        facts: SourceFacts,
        associations: Associations,
        identity_json: str,
        output_dir: str,
        *,
        options_json: str = "{}",
        base: NativeSnapshot | None = None,
        cancel: CancellationToken | None = None,
    ) -> str: ...
    def open(
        self,
        path: str,
        expected_identity_json: str = "{}",
        *,
        cancel: CancellationToken | None = None,
    ) -> NativeSnapshot: ...
    def close(self) -> None: ...

def schemas_json() -> str: ...
def defaults_json() -> str: ...

class ContextResult:
    @property
    def vector(self) -> Array[np.float32]: ...
    @property
    def diagnostics_json(self) -> str: ...

def context_masses(metadata_json: str, options_json: str = "{}") -> str: ...
def aggregate_context(
    raw_query: Array[np.float32] | None,
    context_vectors: Array[np.float32],
    metadata_json: str,
    options_json: str = "{}",
) -> ContextResult: ...

class SegmentResult:
    @property
    def vectors(self) -> Array[np.float32]: ...
    @property
    def ranges_json(self) -> str: ...

def segment(vectors: Array[np.float32], threshold: float | None = None) -> SegmentResult: ...
