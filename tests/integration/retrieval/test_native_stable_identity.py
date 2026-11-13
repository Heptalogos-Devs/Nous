"""Native index identities survive ordinary head deletion and insertion."""

import json
from pathlib import Path
from typing import cast

import numpy as np
import numpy.typing as npt

import nous_native as native
from nous.wave.configuration import WaveConfig
from nous.wave.program import resolve_program


def _search(
    snapshot: native.NativeSnapshot,
    name: str,
    *,
    text: str = "",
    vector: npt.NDArray[np.float32] | None = None,
    options_json: str = "{}",
) -> native.NativeQueryResult:
    program = resolve_program(WaveConfig(), name)
    definition = {
        "nodes": [node.model_dump(mode="json") for node in program.nodes],
        "outputs": {key: ref.model_dump(mode="json") for key, ref in program.outputs.items()},
    }
    return snapshot.execute_program(
        name=name,
        program_json=json.dumps(definition),
        text=text,
        raw_vector=vector,
        contextual_vector=vector,
        options_json=options_json,
    )


def _bundles(
    keys: list[str], vectors: npt.NDArray[np.float32]
) -> tuple[native.Candidates, native.Concepts, native.SourceFacts, native.Associations]:
    count = len(keys)
    u = np.array([], dtype=np.uint64)
    z = np.zeros(count + 1, dtype=np.uint64)
    zero = np.array([0], dtype=np.uint64)
    i = np.array([], dtype=np.int64)
    b = np.array([], dtype=np.bool_)
    f = np.array([], dtype=np.float64)
    metadata = {
        "keys": keys,
        "documents": keys,
        "titles": [""] * count,
        "group_keys": keys,
        "group_rows": list(range(count)),
        "local": [False] * count,
    }
    candidates = native.Candidates(
        json.dumps(metadata),
        [f"common unique{key}" for key in keys],
        np.ascontiguousarray(vectors),
        z,
        u,
        z,
        zero,
        u,
        i,
        z,
        u,
    )
    concepts = native.Concepts(json.dumps({"keys": [], "labels": [], "aliases": []}), None)
    facts = native.SourceFacts(
        json.dumps({"root_keys": [], "root_checksums": [], "sequence_keys": []}), u, zero, u, i, b
    )
    associations = native.Associations(
        json.dumps({"support_groups": [], "keys": [], "relations": []}),
        u,
        u,
        b,
        f,
        f,
        np.array([], dtype=np.int8),
        u,
        zero,
        u,
    )
    return candidates, concepts, facts, associations


def _identity(name: str) -> str:
    return json.dumps(
        {"snapshot_id": name, "space_signature": "space", "build_config_signature": "config"}
    )


def _build(
    engine: native.NativeEngine,
    root: Path,
    name: str,
    keys: list[str],
    vectors: npt.NDArray[np.float32],
    base: native.NativeSnapshot | None = None,
) -> dict[str, object]:
    path = root / name
    candidates, concepts, facts, associations = _bundles(keys, vectors)
    engine.build(candidates, concepts, facts, associations, _identity(name), str(path), base=base)
    return json.loads((path / "timings.json").read_text())


def test_head_changes_keep_old_snapshot_and_incremental_identities(tmp_path: Path) -> None:
    rng = np.random.default_rng(711)
    keys = [f"d{index:04d}" for index in range(1200)]
    vectors = rng.normal(size=(len(keys), 16)).astype(np.float32)
    engine = native.NativeEngine()
    try:
        _build(engine, tmp_path, "old", keys, vectors)
        old = engine.open(str(tmp_path / "old"), _identity("old"))
        deleted_keys, deleted_vectors = keys[1:], vectors[1:]
        deletion = _build(engine, tmp_path, "delete", deleted_keys, deleted_vectors, old)
        lexical = cast(dict[str, object], deletion["lexical_delta"])
        ann = cast(dict[str, object], cast(dict[str, object], deletion["ann_delta"])["candidates"])
        assert lexical["changed"] == 1
        assert ann == {"removed": 1, "added": 0, "rebuilt": False}
        deleted = engine.open(str(tmp_path / "delete"), _identity("delete"))
        _build(engine, tmp_path, "delete-full", deleted_keys, deleted_vectors)
        full = engine.open(str(tmp_path / "delete-full"), _identity("delete-full"))
        options = json.dumps(
            {"grouping": "document", "top_k": 10, "query_space_signature": "space"}
        )
        for query in ("unique" + keys[0], "unique" + keys[-1]):
            old_hits = json.loads(
                _search(old, "lexical", text=query, options_json=options).result_json
            )["hits"]
            incremental_hits = json.loads(
                _search(deleted, "lexical", text=query, options_json=options).result_json
            )["hits"]
            full_hits = json.loads(
                _search(full, "lexical", text=query, options_json=options).result_json
            )["hits"]
            assert [deleted_keys[item["row"]] for item in incremental_hits] == [
                deleted_keys[item["row"]] for item in full_hits
            ]
            assert len(old_hits) == 1
        dense_query = vectors[-1]
        incremental_rows = _search(
            deleted, "dense", vector=dense_query, options_json=options
        ).row_ids
        full_rows = _search(full, "dense", vector=dense_query, options_json=options).row_ids
        assert deleted_keys[int(incremental_rows[0])] == keys[-1]
        assert deleted_keys[int(full_rows[0])] == keys[-1]
        inserted_keys = ["new-head", *deleted_keys]
        inserted_vectors = np.concatenate(
            (rng.normal(size=(1, 16)).astype(np.float32), deleted_vectors)
        )
        insertion = _build(engine, tmp_path, "insert", inserted_keys, inserted_vectors, deleted)
        assert cast(dict[str, object], insertion["lexical_delta"])["changed"] == 1
        assert cast(dict[str, object], insertion["ann_delta"])["candidates"] == {
            "removed": 0,
            "added": 1,
            "rebuilt": False,
        }
        reopened = engine.open(str(tmp_path / "insert"), _identity("insert"))
        _build(engine, tmp_path, "insert-full", inserted_keys, inserted_vectors)
        rebuilt = engine.open(str(tmp_path / "insert-full"), _identity("insert-full"))
        incremental_ties = _search(reopened, "lexical", text="common", options_json=options).row_ids
        full_ties = _search(rebuilt, "lexical", text="common", options_json=options).row_ids
        assert [inserted_keys[int(row)] for row in incremental_ties] == [
            inserted_keys[int(row)] for row in full_ties
        ]
        hit = _search(reopened, "dense", vector=dense_query, options_json=options).row_ids[0]
        assert inserted_keys[int(hit)] == keys[-1]
        assert (
            len(json.loads(_search(old, "lexical", text="unique" + keys[0]).result_json)["hits"])
            == 1
        )
    finally:
        engine.close()
