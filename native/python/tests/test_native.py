"""Public extension contracts across owned buffers, index persistence and grammar."""
import json

import numpy as np
import pytest

from nous_native import CancellationToken, IndexStore, NativeError, NousQLSyntaxError, parse_nousql, schemas_json


def test_owned_dense_lexical_scope_and_reopen(tmp_path):
    vectors = np.array([[1, 0, 0], [0, 1, 0]], dtype=np.float32)
    store = IndexStore(str(tmp_path))
    snapshot = store.build(references=["revision:a", "revision:b"], texts=["人工智能机器学习研究", "山川徒步"], vectors=vectors)
    vectors[:] = -1  # Caller aliases cannot change the adopted generation.
    query = np.array([1, 0, 0], dtype=np.float32)
    rows = np.array([0, 1], dtype=np.uint64)
    assert snapshot.query(mode="dense", vectors=query, allowed_rows=rows).row_ids.tolist() == [0, 1]
    assert snapshot.query(mode="lexical", text="人工智能", allowed_rows=rows).row_ids.tolist() == [0]
    reopened = store.open_snapshot(snapshot.generation)
    assert reopened.query(mode="hybrid", text="人工智能", vectors=query, allowed_rows=np.array([1], dtype=np.uint64)).row_ids.tolist() == [1]
    assert reopened.query(mode="dense", vectors=query, allowed_rows=np.array([], dtype=np.uint64)).row_ids.size == 0
    assert (tmp_path / snapshot.generation / "snapshot.msgpack").is_file()
    with pytest.raises(NativeError):
        reopened.query(mode="dense", vectors=query, allowed_rows=np.array([2], dtype=np.uint64))
    store.close()
    with pytest.raises(NativeError):
        reopened.query(mode="dense", vectors=query, allowed_rows=rows)


def test_explicit_directed_graph_vcp_and_restriction(tmp_path):
    store = IndexStore(str(tmp_path))
    args = dict(
        references=["revision:a", "revision:b"], texts=["学习", "旅行"],
        vectors=np.array([[1, 0, 0], [0, 1, 0]], dtype=np.float32),
        assets_json=json.dumps({"tags": [{"key": "z", "label": "研究"}, {"key": "a", "label": "学习"}, {"key": "isolated", "label": "独立概念"}]}),
        tag_vectors=np.array([[1, 0, 0], [.8, .2, 0], [0, 0, 1]], dtype=np.float32),
        tag_indptr=np.array([0, 2, 3], dtype=np.uint64), tag_indices=np.array([0, 1, 1], dtype=np.uint64),
        graph_indptr=np.array([0, 1, 2, 2], dtype=np.uint64), graph_indices=np.array([1, 0], dtype=np.uint64),
        graph_weights=np.array([1, 2], dtype=np.float64), graph_polarities=np.array([1, -1], dtype=np.int8),
    )
    snapshot = store.build(**args)
    query = args["vectors"][0]
    result = snapshot.query(mode="vcp", vectors=query, allowed_rows=np.array([0, 1], dtype=np.uint64), options_json=json.dumps({"vcp": {"include_trace": True, "thinking": True}}))
    diagnostics = json.loads(result.diagnostics_json)
    assert diagnostics["vcp"]["assets"]["graph"]["graph_source"] == "explicit"
    assert diagnostics["vcp"]["assets"]["graph_edges"] == 1
    assert diagnostics["vcp"]["assets"]["graph"]["negative_edges"] == 1
    assert diagnostics["candidate_traces"]
    reopened = store.open_snapshot(snapshot.generation)
    restricted = reopened.query(mode="vcp", vectors=query, allowed_rows=np.array([0], dtype=np.uint64), allowed_graph_nodes=np.array([0, 2], dtype=np.uint64))
    assert restricted.row_ids.tolist() == [0]
    # Explicit empty graph has no synthetic co-occurrence edges.
    args.update(graph_indptr=np.zeros(4, dtype=np.uint64), graph_indices=np.array([], dtype=np.uint64), graph_weights=np.array([], dtype=np.float64), graph_polarities=np.array([], dtype=np.int8))
    empty = store.build(**args).query(mode="vcp", vectors=query, allowed_rows=np.array([0, 1], dtype=np.uint64))
    assert json.loads(empty.diagnostics_json)["vcp"]["assets"]["graph_edges"] == 0
    token = CancellationToken()
    token.cancel()
    before = set(tmp_path.iterdir())
    with pytest.raises(NativeError) as error:
        store.build(**args, cancel=token)
    assert error.value.code == "cancelled"
    assert set(tmp_path.iterdir()) == before


def test_parser_positions_canonical_and_schemas():
    parsed = parse_nousql('回忆 @entity("Alice") #研究 $limit(5) $time(recorded,last="7d")')
    assert json.loads(parsed.ast_json)["intent"] == "回忆"
    assert parse_nousql(parsed.canonical).ast_json == parsed.ast_json
    with pytest.raises(NousQLSyntaxError) as error:
        parse_nousql('中文 $unknown(1)')
    assert error.value.start_byte == len("中文 ".encode())
    assert error.value.end_byte > error.value.start_byte
    schemas = json.loads(schemas_json())
    assert schemas["query_options"]["properties"]["vcp"]["$ref"].endswith("VcpOptions")


def test_shared_material_roots_keep_distinct_candidate_curves(tmp_path):
    store = IndexStore(str(tmp_path))
    snapshot = store.build(
        references=["memory:a", "association:b"], texts=["研究", "关联"],
        vectors=np.array([[1, 0], [.9, .1]], dtype=np.float32),
        assets_json=json.dumps({"tags": [{"key": "tag:a", "label": "研究"}, {"key": "tag:b", "label": "关联"}]}),
        tag_vectors=np.array([[1, 0], [.8, .2]], dtype=np.float32),
        tag_indptr=np.array([0, 1, 3], dtype=np.uint64), tag_indices=np.array([0, 0, 1], dtype=np.uint64),
        graph_indptr=np.array([0, 1, 1], dtype=np.uint64), graph_indices=np.array([1], dtype=np.uint64), graph_weights=np.array([1.0], dtype=np.float64),
        source_references=["material:shared", "material:independent"],
        candidate_source_indptr=np.array([0, 1, 2], dtype=np.uint64), candidate_source_indices=np.array([0, 0], dtype=np.uint64),
        graph_source_indptr=np.array([0, 1], dtype=np.uint64), graph_source_indices=np.array([1], dtype=np.uint64),
    )
    query = np.array([1, 0], dtype=np.float32)
    result = store.open_snapshot(snapshot.generation).query(mode="vcp", vectors=query, allowed_rows=np.array([0, 1], dtype=np.uint64))
    diagnostics = json.loads(result.diagnostics_json)
    assert diagnostics["vcp"]["assets"]["source_root_count"] == 2
    assert set(result.row_ids.tolist()) == {0, 1}
