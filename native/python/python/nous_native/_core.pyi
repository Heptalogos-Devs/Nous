from collections.abc import Sequence
from typing import Literal

import numpy as np
from numpy.typing import NDArray

class NativeError(RuntimeError):
    code: str
    details_json: str

class NousQLSyntaxError(NativeError):
    start_byte: int
    end_byte: int

class CancellationToken:
    def __init__(self) -> None: ...
    def cancel(self) -> None: ...
    @property
    def cancelled(self) -> bool: ...

class ParsedQuery:
    @property
    def ast_json(self) -> str: ...
    @property
    def canonical(self) -> str: ...

def parse_nousql(source: str) -> ParsedQuery: ...
def nousql_manual() -> str: ...
def schemas_json() -> str: ...

class SearchResult:
    @property
    def row_ids(self) -> NDArray[np.uint64]: ...
    @property
    def scores(self) -> NDArray[np.float64]: ...
    @property
    def generation(self) -> str: ...
    @property
    def diagnostics_json(self) -> str: ...

class Snapshot:
    @property
    def generation(self) -> str: ...
    @property
    def references(self) -> list[str]: ...
    @property
    def capabilities(self) -> list[str]: ...
    def query(
        self,
        *,
        mode: Literal["lexical", "dense", "vcp", "hybrid"],
        text: str = "",
        vectors: NDArray[np.float32] | None = None,
        allowed_rows: NDArray[np.uint64],
        allowed_graph_nodes: NDArray[np.uint64] | None = None,
        options_json: str = "{}",
        cancel: CancellationToken | None = None,
        ghost_vectors: NDArray[np.float32] | None = None,
    ) -> SearchResult: ...

class IndexStore:
    def __init__(self, path: str, options_json: str = "{}") -> None: ...
    def build(
        self,
        *,
        references: Sequence[str],
        texts: Sequence[str],
        assets_json: str = "{}",
        cancel: CancellationToken | None = None,
        vectors: NDArray[np.float32] | None = None,
        tag_vectors: NDArray[np.float32] | None = None,
        tag_indptr: NDArray[np.uint64] | None = None,
        tag_indices: NDArray[np.uint64] | None = None,
        graph_indptr: NDArray[np.uint64] | None = None,
        graph_indices: NDArray[np.uint64] | None = None,
        graph_weights: NDArray[np.float64] | None = None,
        graph_polarities: NDArray[np.int8] | None = None,
        source_references: Sequence[str] | None = None,
        candidate_source_indptr: NDArray[np.uint64] | None = None,
        candidate_source_indices: NDArray[np.uint64] | None = None,
        graph_source_indptr: NDArray[np.uint64] | None = None,
        graph_source_indices: NDArray[np.uint64] | None = None,
    ) -> Snapshot: ...
    def open_snapshot(self, generation: str) -> Snapshot: ...
    def close(self) -> None: ...
