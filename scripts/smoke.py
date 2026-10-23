"""Exercise the current CLI, HTTP and MCP without a model resource.

Usage: python scripts/smoke.py <nous-binary>
The executable copy, fresh instances, datasets, logs, backups and report stay in temp/.
"""
from __future__ import annotations
import argparse
import json
from pathlib import Path
import sys
sys.dont_write_bytecode = True
from smoke_runtime import Runner, Mcp, require


def exercise(runner):
    instance = runner.root / "primary"
    require(runner.cli(instance, "init")["state"] == "initialized", "instance did not initialize")
    require(runner.cli(instance, "init")["state"] == "already_initialized", "initialization did not replay")
    require(runner.cli(instance, "status")["state"] == "stopped", "fresh host is already running")
    require(runner.cli(instance, "config", "validate")["valid"], "bootstrap snapshot is invalid")
    readiness = runner.cli(instance, "model", "check")
    require(all(role["state"] == "waiting_configuration" for role in readiness.values()), "fresh instance has model configuration")
    base = runner.start(instance)
    namespace_receipt = runner.cli(instance, "namespace", "create", "Protocol research", "--operation-id", "namespace")
    namespace = namespace_receipt["result"]["id"]
    require(runner.http(instance, base, "/api/namespaces", {"operation_id": "namespace", "name": "Protocol research"})[1] == namespace_receipt,
            "HTTP and CLI did not share an operation receipt")
    subject = runner.cli(instance, "subject", "create", "--namespace", namespace, "--name", "reader")["result"]["id"]
    first = {"external_id": "coast", "text": "晨星风速站记录海风 NW-COAST-314。", "tags": ["气象"],
             "source": "smoke:coast", "metadata": {"station": "晨星"}, "structured": {"entities": ["风速站"]}}
    second = {"external_id": "rain", "text": "山谷测雨站每周五校准 NW-RAIN-315。", "source": "smoke:rain"}
    content = "\n".join((json.dumps(first, ensure_ascii=False), "not JSON", json.dumps(second, ensure_ascii=False), json.dumps(first, ensure_ascii=False)))
    dataset = runner.runtime / "records.jsonl"
    dataset.write_text(content, encoding="utf-8")
    import_args = ("import", "text", "--namespace", namespace, "--subject", subject, "--file", str(dataset), "--format", "jsonl", "--source", "smoke:records")
    imported = runner.cli(instance, *import_args)
    require((imported["accepted"], imported["replayed"], imported["rejected"]) == (2, 1, 1), "partial import outcomes changed")
    first_result, survivor_result = (imported["items"][index]["receipt"]["result"] for index in (0, 2))
    memory_id, survivor = first_result["memory_id"], survivor_result["memory_id"]
    request = {"namespace_id": namespace, "subject_id": subject, "format": "jsonl", "content": content, "source": "smoke:records"}
    replay = runner.http(instance, base, "/api/imports/text", request)[1]
    require((replay["accepted"], replay["replayed"], replay["rejected"]) == (0, 3, 1), "external identities did not replay")
    changed = dict(first, text="same identity, changed payload")
    rejected = runner.http(instance, base, "/api/imports/text", dict(request, content=json.dumps(changed)))[1]
    require(rejected["items"][0]["error"]["code"] == "Conflict", "changed external payload did not conflict")
    memory = runner.cli(instance, "memory", "get", memory_id)
    require(memory["origin"] == "imported" and memory["model_call_id"] is None, "import invented a model producer")
    source = runner.cli(instance, "memory", "source", memory_id)
    require(source["observation"]["kind"] == "import" and source["model_call"] is None, "import claims an actual contact or model call")
    require(source["material"]["text"] == first["text"] and source["material"]["details"]["structured"] == first["structured"], "source changed imported content")
    mcp = Mcp(runner, instance, base)
    names = {tool["name"] for tool in mcp.send("tools/list")["tools"]}
    require({"query", "cognition", "import_text", "job_cancel", "memory_source"} <= names, "current MCP tools are missing")
    found = mcp.tool("query", {"subject_id": subject, "text": "晨星", "kinds": ["memory"]})
    require(memory_id in {hit["reference"]["id"] for hit in found["hits"]}, "import is not immediately queryable")
    unit = {"content": "灯塔站负责夜间巡检。", "entities": ["灯塔站"], "relations": [{"subject": "灯塔站", "predicate": "负责", "object": "巡检"}]}
    mature = mcp.tool("import_text", dict(request, format="json", content=json.dumps({"paragraphs": [unit]}, ensure_ascii=False), source="smoke:paragraph"))
    mature_id = mature["items"][0]["receipt"]["result"]["memory_id"]
    require(mcp.tool("memory_source", {"memory_id": mature_id})["material"]["details"]["structured"]["payload"] == unit, "structured import lost its payload")
    observation = runner.cli(instance, "observation", "record", "--subject", subject, "--material", first_result["material_id"])["result"]
    require(observation["kind"] == "contact", "explicit observation did not record actual contact")
    formation = runner.cli(instance, "memory", "form", "--subject", subject, "--observation", observation["id"], "--operation-id", "waiting-formation")
    job_id = formation["job_ids"][0]
    runner.wait_job(instance, base, job_id, state="waiting", wait_kind="configuration")
    cancel = runner.http(instance, base, f"/api/jobs/{job_id}/cancel", {"operation_id": "cancel-formation"})[1]
    require(runner.cli(instance, "job", "cancel", job_id, "--operation-id", "cancel-formation") == cancel, "CLI cancellation did not replay")
    require(mcp.tool("job_cancel", {"job_id": job_id, "operation_id": "cancel-formation"}) == cancel, "MCP cancellation differs")
    require(mcp.tool("job_get", {"job_id": job_id})["job"]["state"] == "cancelled", "task did not cancel")
    runner.report["checks"].update(initialization=True, shared_receipts=True, partial_import=True, source_preserved=True,
                                    external_id_conflict=True, typed_query=True, mcp_http=True, contact_identity=True, cancel_all_protocols=True)
    mcp.close()
    runner.stop(instance)
    scoped_file = Path(runner.cli(instance, "token", "create", "--namespace", namespace, "--subject", subject, "--name", "reader")["token_file"])
    base = runner.start(instance)
    denied = runner.http(instance, base, "/api/status", token_file=scoped_file, expected=403)[1]
    require(denied["error"]["code"] == "Forbidden", "subject token read administrator status")
    require(runner.cli(instance, "memory", "source", survivor, token_file=scoped_file)["material"]["text"] == second["text"], "scoped source read failed")
    with_stdio = Mcp(runner, instance, base, stdio=True)
    try:
        require(with_stdio.tool("query", {"subject_id": subject, "text": "NW-RAIN"})["hits"], "stdio query omitted imported cognition")
        require(with_stdio.tool("memory_source", {"memory_id": survivor})["memory"]["id"] == survivor, "stdio source selected another identity")
    finally:
        with_stdio.close()
    memory = runner.cli(instance, "memory", "get", memory_id)
    frozen = runner.cli(instance, "memory", "change", memory_id, "--action", "freeze", "--expected-revision", str(memory["revision"]), "--operation-id", "freeze")["result"]
    require(not runner.cli(instance, "query", "--subject", subject, "晨星")["hits"], "frozen memory remains active")
    require(runner.cli(instance, "query", "--subject", subject, "--include-inactive", "晨星")["hits"], "inactive retrieval omitted retained memory")
    restored = runner.cli(instance, "memory", "change", memory_id, "--action", "restore", "--expected-revision", str(frozen["revision"]), "--operation-id", "restore")["result"]
    trashed = runner.cli(instance, "memory", "change", memory_id, "--action", "trash", "--expected-revision", str(restored["revision"]), "--operation-id", "trash")["result"]
    require(memory_id in {item["id"] for item in runner.cli(instance, "memory", "recycle-bin", "--subject", subject)}, "trash did not enter recycle bin")
    runner.cli(instance, "memory", "change", memory_id, "--action", "purge", "--force", "--expected-revision", str(trashed["revision"]), "--operation-id", "purge")
    require(not runner.cli(instance, "query", "--subject", subject, "--include-inactive", "晨星")["hits"], "purged cognition remains queryable")
    require(runner.cli(instance, "source", "read", first_result["material_id"])["text"] == first["text"], "purge changed original material")
    current = runner.cli(instance, "memory", "get", survivor)
    reference = f"memory:{survivor}@{current['revision']}"
    exact = runner.cli(instance, "query", "--subject", subject, "--reference", reference)
    require(exact["hits"][0]["reference"] == {"kind": "memory", "id": survivor, "revision": current["revision"]}, "CLI exact reference changed identity")
    backup = runner.cli(instance, "backup", "create")["backup"]
    require(runner.call_count(instance) == 0, "unconfigured workflow created a model call")
    runner.stop(instance)
    base = runner.start(instance)
    require(runner.cli(instance, "job", "get", job_id)["job"]["state"] == "cancelled", "restart resumed cancelled work")
    require(runner.cli(instance, "query", "--subject", subject, "--reference", reference)["hits"][0]["object"] == current, "restart changed exact cognition")
    old_token = runner.token(instance)
    runner.stop(instance)
    target = runner.root / "restored"
    runner.cli(target, "backup", "restore", backup)
    require(runner.token(target) != old_token, "restore reused administrator credentials")
    base = runner.start(target)
    require(runner.cli(target, "query", "--subject", subject, "--reference", reference)["hits"][0]["object"] == current, "snapshot restore changed cognition")
    require(runner.cli(target, "memory", "source", survivor)["material"]["text"] == second["text"], "snapshot restore changed original source")
    runner.stop(target)
    fixtures = Path(__file__).resolve().parents[1] / "crates/nous-app/tests/fixtures/bundles"
    offline = runner.root / "offline"
    for filename in ("v1-knowledge-1.0.0.amembundle", "v1-full-1.0.0.amembundle", "v2-images-1.0.0.amembundle"):
        inspected = runner.cli(offline, "import", "bundle", "inspect", "--file", str(fixtures / filename), "--details")
        require(inspected["knowledge"]["docs"] and inspected["members"], "offline bundle inspection lost records")
    require(not offline.exists(), "offline bundle inspection initialized an instance")
    runner.report["checks"].update(scoped_token=True, mcp_stdio=True, lifecycle=True, purge=True, restart=True,
                                    snapshot_restore=True, new_administrator=True, offline_bundles=True)
    runner.report.update(namespace_id=namespace, subject_id=subject, cancelled_job=job_id, surviving_reference=reference, model_calls=0)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    runner = Runner(args.binary.resolve(strict=True))
    failure = None
    try:
        exercise(runner)
    except Exception as error:
        failure = error
    finally:
        runner.close()
        report = runner.save(failure)
    print(json.dumps({"passed": failure is None, "report": str(report)}, ensure_ascii=False))
    if failure:
        raise failure


if __name__ == "__main__":
    main()
