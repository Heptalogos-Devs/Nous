# 目标 API 草案：由 native 实现者与现有 kernel 对接；不是已实现的扩展。
from collections.abc import Sequence
from typing import Literal

import numpy as np
from numpy.typing import NDArray

class NativeError(Exception):
    @property
    def code(self) -> str: ...
    @property
    def details_json(self) -> str: ...

class NousQLSyntaxError(NativeError):
    @property
    def start_byte(self) -> int: ...
    @property
    def end_byte(self) -> int: ...

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
    def query(
        self,
        *,
        mode: Literal["lexical", "dense", "vcp", "hybrid"],
        text: str,
        vectors: NDArray[np.float32] | None,
        allowed_rows: NDArray[np.uint64],
        allowed_graph_nodes: NDArray[np.uint64],
        options_json: str,
        cancel: CancellationToken,
    ) -> SearchResult: ...

class IndexStore:
    def __init__(self, path: str, options_json: str) -> None: ...
    def build(
        self,
        *,
        references: Sequence[str],
        texts: Sequence[str],
        assets_json: str,
        cancel: CancellationToken,
        vectors: NDArray[np.float32] | None = None,
        graph_indptr: NDArray[np.uint64] | None = None,
        graph_indices: NDArray[np.uint64] | None = None,
        graph_weights: NDArray[np.float64] | None = None,
        epa_assets: NDArray[np.float64] | None = None,
    ) -> Snapshot: ...
    def open_snapshot(self, generation: str) -> Snapshot: ...
    def close(self) -> None: ...
