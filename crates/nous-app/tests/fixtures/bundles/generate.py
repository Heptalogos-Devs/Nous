"""Regenerate goldens through unchanged upstream A_Memorix exporter functions.

Requires Python with NumPy and a local MaiBot checkout at SOURCE_COMMIT.
Only row selection, vector retrieval, asset reads and clocks use fixture adapters.
"""
# SPDX-License-Identifier: AGPL-3.0-only; see ../../../licenses/A_memorix-LICENSE.
import argparse
import ast
import asyncio
from collections import defaultdict
from datetime import datetime, timezone
from io import BytesIO
from pathlib import Path
import base64
import copy
import hashlib
import json
import re
import random
import struct
import subprocess
import sys
from types import SimpleNamespace
import zipfile

sys.dont_write_bytecode = True
import numpy as np

SOURCE_COMMIT = "c602a2780f4f4dda4a6be46d67abc1cfb1ad420c"
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--upstream-repo", type=Path, required=True)
parser.add_argument("--output-dir", type=Path, default=Path(__file__).parent)
arguments = parser.parse_args()
repository = arguments.upstream_repo.resolve()
commit = subprocess.check_output(["git", "-C", str(repository), "rev-parse", "HEAD"], text=True).strip()
if commit != SOURCE_COMMIT:
    raise SystemExit(f"Expected MaiBot {SOURCE_COMMIT}, found {commit}")
source = repository / "src/A_memorix/core/runtime/services/bundle_admin_service.py"
output_dir = arguments.output_dir.resolve()
output_dir.mkdir(parents=True, exist_ok=True)
tree = ast.parse(source.read_text(encoding="utf-8"))
helpers = {"_canonical_json_bytes", "_canonical_image_evidence", "_canonical_image_semantics", "_json_object", "_json_list"}
methods = {"_export_bundle", "_build_knowledge_docs", "_export_vector_member", "_safe_file_stem"}
body = [ast.ImportFrom(module="__future__", names=[ast.alias(name="annotations")], level=0)]
for node in tree.body:
    if isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id in {"BUNDLE_FORMAT", "BUNDLE_FORMAT_VERSION", "BUNDLE_LEGACY_FORMAT_VERSION", "BUNDLE_EXTENSION", "FULL_STATE_EXCLUSIONS", "FULL_STATE_TABLE_ORDER"} for target in node.targets):
        body.append(node)
    if isinstance(node, ast.FunctionDef) and node.name in helpers:
        body.append(node)
    if isinstance(node, ast.ClassDef) and node.name == "MemoryBundleAdminService":
        selected = copy.deepcopy(node)
        selected.bases = []
        selected.body = [item for item in selected.body if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)) and item.name in methods]
        body.append(selected)
namespace = dict(globals())

class FixtureDatetime(datetime):
    @classmethod
    def now(cls, tz=None):
        return cls(2026, 10, 23, 0, 0, 0, tzinfo=timezone.utc)

class FixtureZipFile(zipfile.ZipFile):
    def writestr(self, name, data, compress_type=None, compresslevel=None):
        if isinstance(name, str):
            name = zipfile.ZipInfo(name, date_time=(2026, 10, 23, 0, 0, 0))
        return super().writestr(name, data, compress_type=self.compression, compresslevel=6)

namespace["datetime"] = FixtureDatetime
namespace["zipfile"] = SimpleNamespace(ZipFile=FixtureZipFile, ZIP_DEFLATED=zipfile.ZIP_DEFLATED)
exec(compile(ast.fix_missing_locations(ast.Module(body=body, type_ignores=[])), str(source), "exec"), namespace)

def sha(text):
    return hashlib.sha256(text.encode()).hexdigest()

text = "潮汐站在周三升起蓝色旗帜。"
paragraph = sha(text)
entity_a, entity_b = sha("潮汐站"), sha("蓝色旗帜")
relation = sha("潮汐站|使用|蓝色旗帜")
tables = {name: [] for name in namespace["FULL_STATE_TABLE_ORDER"]}
tables.update({
    "paragraphs": [{"hash": paragraph, "content": text, "source": "chat_summary:old", "created_at": 1e-7, "updated_at": 0.0001, "knowledge_type": "factual", "time_confidence": 1e-7, "metadata": "{\"person_ids\":[\"p1\"],\"chat_id\":\"old\"}"}],
    "entities": [{"hash": entity_a, "name": "潮汐站"}, {"hash": entity_b, "name": "蓝色旗帜"}],
    "relations": [{"hash": relation, "subject": "潮汐站", "predicate": "使用", "object": "蓝色旗帜", "source_paragraph": paragraph, "retention_strength": 1.0}],
    "paragraph_entities": [{"paragraph_hash": paragraph, "entity_hash": entity_a}, {"paragraph_hash": paragraph, "entity_hash": entity_b}],
    "paragraph_relations": [{"paragraph_hash": paragraph, "relation_hash": relation}],
    "external_memory_refs": [{"external_id": "fixture:paragraph", "paragraph_hash": paragraph}],
    "episodes": [{"episode_id": "episode-1", "title": "周三观测", "source": "chat_summary:old", "evidence_ids_json": json.dumps([paragraph])}],
    "episode_paragraphs": [{"episode_id": "episode-1", "paragraph_hash": paragraph}],
    "fact_claims": [{"claim_id": "claim-1", "scope_type": "chat", "scope_id": "old", "fact_key": "station.flag", "value_text": "蓝色旗帜"}],
    "fact_evidence": [{"claim_id": "claim-1", "evidence_type": "paragraph", "evidence_id": paragraph, "stance": "support"}],
    "fact_transitions": [{"transition_id": 1, "old_claim_id": None, "new_claim_id": "claim-1", "evidence_id": paragraph}],
    "person_profile_snapshots": [{"snapshot_id": 1, "person_id": "p1", "profile_version": 1, "profile_text": "负责潮汐观测", "evidence_ids_json": json.dumps([paragraph]), "fact_claim_ids_json": "[\"claim-1\"]"}],
    "person_profile_overrides": [{"person_id": "p1", "profile_text": "观测员"}],
    "person_profile_alias_overrides": [{"person_id": "p1", "aliases_json": "[\"甲\"]"}],
})
png = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6iWQAAAAASUVORK5CYII=")
image_hash = hashlib.sha256(png).hexdigest()
records = {
    "assets": [{"asset_id": "asset-1", "content_hash": image_hash, "storage_key": "source.png", "byte_size": len(png), "mime_type": "image/png", "width": 1, "height": 1, "status": "active"}],
    "occurrences": [{"occurrence_id": "occ-1", "asset_id": "asset-1", "component_path": "0", "chat_id": "old", "message_id": "message-1", "occurred_at": 1e-7}],
    "observations": [{"observation_id": "obs-1", "occurrence_id": "occ-1", "text": "蓝色旗帜", "source_kind": "user_statement", "confirm_status": "confirmed", "evidence_json": "{\"chat_id\":\"old\",\"confidence\":1e-07,\"visible\":true}"}],
    "links": [{"link_id": "link-1", "occurrence_id": "occ-1", "target_type": "entity", "target_id": entity_a, "link_kind": "fact_evidence", "evidence_json": "{\"fact_id\":\"claim-1\",\"source_link_id\":\"old-link\"}"}],
    "fingerprint": {"hash": "fixture-image", "dimension": 2},
}

class Vectors:
    def get_vectors(self, ids):
        return {identity: np.asarray([0.25, 0.75], dtype=np.float32) for identity in ids}

class ImageRuntime:
    asset_store = None
    def __init__(self):
        self.asset_store = self
    def read_bytes(self, storage_key):
        assert storage_key == "source.png"
        return png
    def export_records(self, ids):
        assert ids == ["occ-1"]
        return copy.deepcopy(records)
    def export_vectors(self, ids):
        output = BytesIO()
        np.savez_compressed(output, ids=np.asarray(ids), vectors=np.asarray([[0.25, 0.75] for _ in ids], dtype=np.float32))
        return output.getvalue()

class Exporter(namespace["MemoryBundleAdminService"]):
    def __init__(self, images=False):
        self.images = images
        self.metadata_store = self
    async def _select_paragraphs(self, selector):
        return tables["paragraphs"], {"type": "all", "values": [], "precision": "exact"}
    def _select_image_occurrence_ids(self, **kwargs):
        return ["occ-1"] if self.images else []
    def list_image_memory_links(self, ids):
        return records["links"] if ids else []
    def _query_by_values(self, table, column, values):
        return [row for row in tables[table] if row[column] in values]
    def _collect_related_rows(self, paragraphs):
        return tables
    def _image_runtime(self):
        return ImageRuntime() if self.images else None
    def _graph_vector_id(self, kind, identity):
        return f"{kind}:{identity}"
    def _dual_vector_pools_enabled(self):
        return True
    def _paragraph_store(self):
        return Vectors()
    def _graph_vector_store(self):
        return Vectors()
    def _stored_embedding_fingerprint(self, store):
        return {"hash": "fixture-text", "dimension": 2}
    def _bundles_root(self):
        return output_dir

async def main():
    produced = []
    for label, level, images in [("v1-knowledge", "knowledge", False), ("v1-full", "full", False), ("v2-images", "full", True)]:
        # The original exporter chooses a new filename on collision. Removing only
        # our three named outputs keeps regeneration stable without touching other files.
        expected_path = output_dir / f"{label}-1.0.0.amembundle"
        expected_path.unlink(missing_ok=True)
        result = await Exporter(images)._export_bundle({"content_level": level, "include_vectors": True, "include_image_related": True, "package": {"id": "golden." + label, "version": "1.0.0", "name": label}})
        assert result["success"]
        item = {"file": expected_path.name, "semantic_digest": result["manifest"]["content_digest"], "sha256": hashlib.sha256(expected_path.read_bytes()).hexdigest(), "version": result["manifest"]["format_version"]}
        produced.append(item)
        print(json.dumps(item))
    randomizer = random.Random(20261023)
    values = [0.0, -0.0, 1.0, 1e-7, 0.0001, 1e15, 1e16, 1e20, 1e23, 5e-324, 1.7976931348623157e308]
    for _ in range(256):
        value = struct.unpack("!d", randomizer.getrandbits(64).to_bytes(8, "big"))[0]
        if np.isfinite(value):
            values.append(value)
    cases = [
        {"unicode": "潮汐\u2028站🧠", "controls": "\b\t\n\f\r\x00", "b": 1, "a": {"𝄞": "\"\\/"}},
        *values,
    ]
    canonical_cases = [{"input": json.dumps(value, ensure_ascii=True), "canonical": namespace["_canonical_json_bytes"](value).decode("utf-8")} for value in cases]
    (output_dir / "canonical-cases.json").write_text(json.dumps(canonical_cases, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    provenance = {"source": "MaiBot/src/A_memorix/core/runtime/services/bundle_admin_service.py", "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(), "commit": SOURCE_COMMIT, "actual_upstream_methods": sorted(methods), "actual_upstream_helpers": sorted(helpers), "fixtures": produced, "canonical_cases": len(canonical_cases)}
    (output_dir / "sources.json").write_text(json.dumps(provenance, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

asyncio.run(main())
