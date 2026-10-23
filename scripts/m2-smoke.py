"""Exercise M2 imports, lifecycle, maintenance and purge through real CLI/HTTP/MCP.

Usage: python scripts/m2-smoke.py <nous-binary>
Creates a fresh instance without model configuration, copies the executable into
project temp/runtime/, and retains its logs, dataset and report under temp/.
"""
from __future__ import annotations

import argparse
import contextlib
import importlib.util
import json
from pathlib import Path
import sqlite3
import sys
import time


def load_helpers():
    # Reuse the actual M1 process lifecycle and MCP transport implementation.
    # Importing this helper never runs its paid-model exercise.
    sys.dont_write_bytecode = True
    path = Path(__file__).with_name("m1-smoke.py")
    spec = importlib.util.spec_from_file_location("nous_smoke_helpers", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


helpers = load_helpers()
require = helpers.require


def exercise(runner):
    instance = runner.root / "instance"
    runner.cli(instance, "init")
    require(runner.cli(instance, "model", "check")["state"] == "waiting_configuration",
            "fresh M2 instance unexpectedly has a model resource")
    base = runner.start(instance)
    namespace = runner.cli(instance, "namespace", "create", "M2 source imports")["result"]["id"]
    subject = runner.cli(instance, "subject", "create", "--namespace", namespace, "--name", "reader")["result"]["id"]
    first = {"external_id": "coast", "text": "海风推动风筝，观测员每周三记录风速。", "source": "m2:coast",
             "source_type": "document", "tags": ["气象"], "metadata": {"station": "晨星"}}
    second = {"external_id": "valley", "text": "山谷中的测雨站每周五校准一次，负责人是许澄。"}
    content = "\n".join((json.dumps(first, ensure_ascii=False), "not JSON", json.dumps(second, ensure_ascii=False), json.dumps(first, ensure_ascii=False)))
    dataset = runner.runtime / "records.jsonl"
    dataset.write_text(content, encoding="utf-8")
    import_args = ("import", "text", "--namespace", namespace, "--subject", subject,
                   "--file", str(dataset), "--format", "jsonl", "--source", "m2:jsonl")
    imported = runner.cli(instance, *import_args)
    require((imported["accepted"], imported["replayed"], imported["rejected"]) == (2, 1, 1),
            "JSONL did not retain accepted/replayed/rejected item outcomes")
    memory_id = imported["items"][0]["receipt"]["result"]["memory_id"]
    survivor_id = imported["items"][2]["receipt"]["result"]["memory_id"]
    require(imported["items"][3]["receipt"] == imported["items"][0]["receipt"], "duplicate external ID changed its original receipt")
    runner.report.update(namespace_id=namespace, subject_id=subject, memory_id=memory_id, survivor_id=survivor_id)
    request = {"namespace_id": namespace, "subject_id": subject, "format": "jsonl", "content": content, "source": "m2:jsonl"}
    replay = runner.http(instance, base, "/v1/imports/text", request)[1]
    require((replay["accepted"], replay["replayed"], replay["rejected"]) == (0, 3, 1), "HTTP import did not replay the same external identities")
    conflict_record = dict(first, text="同一外部 ID 对应的内容已经改变。")
    conflict = runner.http(instance, base, "/v1/imports/text", dict(request, format="json", content=json.dumps([conflict_record], ensure_ascii=False)))[1]
    require(conflict["rejected"] == 1 and conflict["items"][0]["error"]["code"] == "Conflict", "changed external-ID payload was not rejected as Conflict")
    memory = runner.cli(instance, "memory", "get", memory_id)
    require(memory["origin"] == "imported" and memory["model_call_id"] is None and memory["prompt_version"] is None,
            "faithful imported memory incorrectly claims a model producer")
    runner.query(instance, subject, memory)
    source = runner.cli(instance, "memory", "source", memory_id)
    require(source["observation"]["kind"] == "import", "import incorrectly claims an actual contact event")
    require(source["model_call"] is None and source["material"]["text"] == first["text"], "imported source requires a model call or changed original text")
    require(source["material"]["details"]["metadata"] == first["metadata"] and source["material"]["details"]["tags"] == first["tags"], "import lost source metadata or tags")
    mcp = helpers.Mcp(runner, instance, base)
    names = {tool["name"] for tool in mcp.send("tools/list")["tools"]}
    require({"import_text", "memory_change", "memory_recycle_bin", "memory_audit", "memory_maintain"} <= names, "M2 MCP tools are missing")
    unit = {"content": "灯塔站负责夜间巡检，每次巡检保留完整来源。", "entities": ["灯塔站"],
            "relations": [{"subject": "灯塔站", "predicate": "负责", "object": "夜间巡检"}],
            "knowledge_type": "factual", "time_meta": {"event_time": 123}}
    paragraph_doc = {"paragraphs": [unit], "entities": [{"name": "文档实体"}]}
    structured_request = dict(request, format="json", content=json.dumps(paragraph_doc, ensure_ascii=False), source="m2:paragraph")
    structured = mcp.tool("import_text", structured_request)
    structured_result = structured["items"][0]["receipt"]["result"]
    structured_id = structured_result["memory_id"]
    require(structured_result["structured_preserved"], "paragraph import did not mark preserved structure")
    details = mcp.tool("memory_source", {"memory_id": structured_id})["material"]["details"]["structured"]
    require(details["payload"] == unit and details["document_context"]["entities"] == paragraph_doc["entities"], "paragraph import discarded structured provenance")
    canonical_replay = mcp.tool("import_text", dict(structured_request, content=json.dumps(paragraph_doc, ensure_ascii=False, sort_keys=True)))
    require(canonical_replay["replayed"] == 1 and canonical_replay["accepted"] == 0, "JSON key reordering changed automatic import identity")
    lpmm = {"passage": "港湾的潮位记录由陈洛维护。", "extracted_entities": ["港湾", "陈洛"], "extracted_triples": [["陈洛", "维护", "潮位记录"]]}
    lpmm_result = mcp.tool("import_text", dict(request, format="json", content=json.dumps({"docs": [lpmm]}, ensure_ascii=False), source="m2:lpmm"))
    lpmm_id = lpmm_result["items"][0]["receipt"]["result"]["memory_id"]
    require(mcp.tool("memory_source", {"memory_id": lpmm_id})["material"]["details"]["structured"]["payload"] == lpmm,
            "LPMM import discarded extracted entities or triples")
    runner.report["checks"].update(partial_jsonl=True, external_id_replay=True, payload_conflict=True,
                                   imported_without_model=True, immediate_query=True, faithful_source=True,
                                   structured_paragraphs=True, structured_lpmm=True, canonical_identity=True, mcp_import=True)

    # Issue only test-instance credentials, while that instance is stopped.
    runner.stop(instance)
    namespace_token = Path(runner.cli(instance, "token", "create", "--namespace", namespace, "--name", "namespace-manager")["token_file"])
    subject_token = Path(runner.cli(instance, "token", "create", "--namespace", namespace, "--subject", subject, "--name", "reader")["token_file"])
    base = runner.start(instance)
    mcp = helpers.Mcp(runner, instance, base)
    current = runner.cli(instance, "memory", "get", memory_id)

    def change(action, **fields):
        nonlocal current
        arguments = {"memory_id": memory_id, "operation_id": "m2-" + action,
                     "expected_revision": current["revision"], "action": action, "reason": "M2 protocol smoke"}
        arguments.update(fields)
        receipt = mcp.tool("memory_change", arguments)
        current = receipt["result"]
        return receipt, arguments

    def query_ids(include_inactive=False):
        result = mcp.tool("query", {"subject_id": subject, "text": "春潮", "include_inactive": include_inactive})
        return {hit["memory"]["id"] for hit in result["hits"]}

    revised_receipt, revised_request = change("revise", edit={"title": "海风观测", "content": "海风推动蓝色风筝，风速站在周三记录春潮。", "epistemic_status": "reported"})
    require(memory_id in query_ids(), "revised text is not immediately queryable")
    require(mcp.tool("memory_source", {"memory_id": memory_id})["material"]["text"] == first["text"], "revision changed immutable source material")
    protected = runner.cli(instance, "--token-file", str(namespace_token), "memory", "change", memory_id,
                           "--action", "protect", "--expected-revision", str(current["revision"]),
                           "--operation-id", "m2-protect", "--reason", "M2 protocol smoke")
    current = protected["result"]
    require(current["lifecycle"]["pinned"], "protect without hours did not pin the memory")
    refused = mcp.send("tools/call", {"name": "memory_change", "arguments": {"memory_id": memory_id,
                       "operation_id": "m2-protected-trash", "action": "trash", "expected_revision": current["revision"]}})
    require(refused.get("isError") and refused["structuredContent"]["error"]["code"] == "Conflict", "protected memory was trashed without explicit force")
    change("freeze")
    require(current["lifecycle"]["activation"] == "frozen" and memory_id not in query_ids(), "frozen memory remained active in retrieval")
    inactive = runner.cli(instance, "query", "--subject", subject, "--include-inactive", "春潮")
    require(memory_id in {hit["memory"]["id"] for hit in inactive["hits"]}, "include_inactive did not expose a frozen memory")

    # Put protected/frozen records at the beginning of the actual stable-ID scan.
    all_ids = sorted((memory_id, survivor_id, structured_id, lpmm_id))
    for index, action in ((0, "protect"), (1, "freeze")):
        entry = mcp.tool("memory_get", {"memory_id": all_ids[index]})
        mcp.tool("memory_change", {"memory_id": entry["id"], "operation_id": "m2-scan-prefix-" + action,
                 "action": action, "expected_revision": entry["revision"], "reason": "maintenance pagination prefix"})
    after, snapshot, scanned, pages = None, None, 0, []
    while True:
        arguments = ["memory", "maintain", "--subject", subject, "--limit", "1", "--operation-id", f"m2-page-{len(pages)}"]
        if after is not None:
            arguments += ["--after", after, "--snapshot-seq", str(snapshot)]
        page_receipt = runner.cli(instance, *arguments)
        page = page_receipt["result"]
        if snapshot is None:
            snapshot = page["snapshot_seq"]
        require(page["snapshot_seq"] == snapshot, "maintenance page changed snapshot sequence")
        scanned += page["scanned"]
        pages.append(page)
        if page["complete"]:
            break
        require(page["next_cursor"] and page["next_cursor"] != after and len(pages) <= len(all_ids), "maintenance cursor failed to advance")
        after = page["next_cursor"]
    require(len(pages) > 1 and scanned == len(all_ids), "bounded maintenance did not scan beyond protected/frozen prefix")
    require(runner.cli(instance, *arguments) == page_receipt, "maintenance page receipt was not idempotent")
    current = mcp.tool("memory_get", {"memory_id": memory_id})
    change("reinforce", strength=1.0)
    require(current["lifecycle"]["activation"] == "active" and current["lifecycle"]["last_reinforced_unix_ms"] is not None,
            "reinforcement did not revive the frozen memory")
    change("forget")
    require(current["lifecycle"]["activation"] == "forgotten" and memory_id not in query_ids(), "forget did not affect active retrieval")
    require(memory_id in query_ids(True), "forgotten memory was unavailable to explicit inactive retrieval")
    change("restore")
    require(current["lifecycle"]["activation"] == "active" and memory_id in query_ids(), "restore did not restore active retrieval")
    trashed_receipt, _ = change("trash")
    recycle = runner.cli(instance, "memory", "recycle-bin", "--subject", subject)
    require(memory_id in {entry["id"] for entry in recycle}, "trash did not enter the recycle bin")
    require(memory_id not in query_ids(True), "trash appeared even with inactive retrieval")
    # Restore with a fresh operation identity; the earlier restore receipt remains immutable.
    restored = mcp.tool("memory_change", {"memory_id": memory_id, "operation_id": "m2-restore-trash",
                         "action": "restore", "expected_revision": current["revision"], "reason": "M2 protocol smoke"})
    current = restored["result"]
    audit = mcp.tool("memory_audit", {"memory_id": memory_id, "limit": 100})
    require({"revise", "protect", "freeze", "reinforce", "forget", "restore", "trash"} <= {entry["action"] for entry in audit},
            "audit omitted lifecycle operations")
    history = mcp.tool("memory_history", {"memory_id": memory_id})
    require(history[0]["content"] == first["text"] and len(history) >= 8, "history lost the original imported revision")
    bridge = helpers.Mcp(runner, instance, base, stdio=True)
    try:
        require(bridge.tool("memory_audit", {"memory_id": memory_id, "limit": 100}) == audit, "stdio audit differs from HTTP MCP")
    finally:
        bridge.close()
    runner.report["checks"].update(revise=True, protect=True, freeze=True, include_inactive=True,
                                   maintenance_continuation=True, reinforce=True, forget=True, restore=True,
                                   trash_recycle_bin=True, audit=True, retained_history=True, mcp_stdio=True)
    runner.report["maintenance_pages"] = pages

    before_restart = current
    runner.stop(instance)
    base = runner.start(instance)
    mcp = helpers.Mcp(runner, instance, base)
    require(mcp.tool("memory_get", {"memory_id": memory_id}) == before_restart, "restart changed memory lifecycle or revision")
    require(mcp.tool("memory_change", revised_request) == revised_receipt, "revision receipt did not replay after restart")
    require(mcp.tool("memory_get", {"memory_id": memory_id}) == before_restart, "old receipt replay overwrote current revision")
    replay = runner.cli(instance, *import_args)
    require((replay["accepted"], replay["replayed"], replay["rejected"]) == (0, 3, 1), "import replay changed after restart")

    # Ensure the memory was actually indexed before testing removal through the authority path.
    deadline = time.monotonic() + 15
    while True:
        coverage = runner.cli(instance, "status")["index"]
        if coverage["through_commit_seq"] >= before_restart["commit_seq"]:
            break
        require(time.monotonic() < deadline, "index never covered the memory before purge")
        time.sleep(0.1)
    trashed = mcp.tool("memory_change", {"memory_id": memory_id, "operation_id": "m2-trash-for-purge",
                       "action": "trash", "expected_revision": before_restart["revision"], "reason": "M2 purge smoke"})
    purge_request = {"memory_id": memory_id, "operation_id": "m2-force-purge", "action": "purge",
                     "expected_revision": trashed["result"]["revision"], "force": True, "reason": "M2 purge smoke"}
    mcp.headers["Authorization"] = "Bearer " + subject_token.read_text().strip()
    denied = mcp.send("tools/call", {"name": "memory_change", "arguments": purge_request})
    require(denied.get("isError") and denied["structuredContent"]["error"]["code"] == "Forbidden", "subject credential performed a namespace purge")
    mcp.headers["Authorization"] = "Bearer " + namespace_token.read_text().strip()
    purged = mcp.tool("memory_change", purge_request)
    require(purged["result"]["purged"], "namespace force purge did not complete")
    require(memory_id not in query_ids(True), "purged indexed candidate remained visible")
    missing = mcp.send("tools/call", {"name": "memory_get", "arguments": {"memory_id": memory_id}})
    require(missing.get("isError") and missing["structuredContent"]["error"]["code"] == "NotFound", "purged memory can still be read")
    require(mcp.tool("memory_change", purge_request) == purged, "purge receipt did not replay")
    require("purge" in {entry["action"] for entry in mcp.tool("memory_audit", {"memory_id": memory_id, "limit": 100})}, "purge removed its audit trail")
    runner.stop(instance)
    base = runner.start(instance)
    require(runner.cli(instance, "memory", "get", memory_id, allow_failure=True) is None, "restart resurrected a purged memory")
    survivor = runner.cli(instance, "memory", "get", survivor_id)
    require(survivor["origin"] == "imported" and survivor["model_call_id"] is None, "restart changed imported producer semantics")
    found = runner.cli(instance, "query", "--subject", subject, "--include-inactive", "--reference", survivor_id)
    require(survivor_id in {hit["memory"]["id"] for hit in found["hits"]}, "restart lost a surviving imported memory")
    require(runner.cli(instance, "memory", "source", structured_id)["material"]["details"]["structured"]["payload"] == unit,
            "restart lost structured provenance")
    uri = (instance / "data" / "authority.sqlite").resolve().as_uri() + "?mode=ro"
    with contextlib.closing(sqlite3.connect(uri, uri=True)) as database:
        calls = database.execute("SELECT count(*) FROM model_calls").fetchone()[0]
    require(calls == 0, "no-model import/lifecycle smoke unexpectedly created model calls")
    runner.stop(instance)
    runner.report["checks"].update(restart=True, durable_receipt_replay=True, namespace_purge_authority=True,
                                   forced_purge=True, purged_candidate_removed=True, purge_restart=True,
                                   surviving_ids=True, structured_restart=True, zero_model_calls=True)
    runner.report.update(structured_memory_id=structured_id, lpmm_memory_id=lpmm_id,
                         initial_index_coverage=coverage, purge_commit_seq=purged["commit_seq"],
                         purged_memory_id=memory_id, model_calls=calls)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    runner = helpers.Runner(args.binary.resolve(strict=True), Path(__file__).resolve().parents[1], 30, label="m2-smoke")
    try:
        exercise(runner)
    except Exception as error:
        report = runner.save(error)
        raise SystemExit(f"M2 smoke failed: {error}\nReport: {report}") from None
    finally:
        runner.close()
    report = runner.save()
    print(json.dumps({"passed": True, "checks": runner.report["checks"], "report": str(report)}, ensure_ascii=False))


if __name__ == "__main__":
    main()
