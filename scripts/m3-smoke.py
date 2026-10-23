"""Exercise real text embeddings, dense/VCP queries and durable recovery.

Usage: python scripts/m3-smoke.py <nous-binary> --instance <configured-template>
Only the template's model resource location and protected credential are read.
The script creates an isolated instance, executable copy, forwarding proxy, logs
and report under project temp/. It makes real, budgeted embedding requests.
"""
from __future__ import annotations

import argparse
import contextlib
from http.client import HTTPConnection, HTTPSConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import sys
import threading
import time
import tomllib
from urllib.parse import urlsplit


sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("nous_m3_helpers", Path(__file__).with_name("m1-smoke.py"))
helpers = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helpers)
require = helpers.require


class DelayedGateway:
    """Forward exact provider bytes; one request can pause before dispatch.

    A paused request is abandoned after the test host is killed. No replacement
    provider output is generated, and restart reaches the real provider normally.
    """

    def __init__(self, upstream, runtime):
        target = urlsplit(upstream)
        require(target.scheme in {"http", "https"} and target.hostname and not target.username
                and not target.password and not target.query and not target.fragment
                and target.path.endswith("/"), "template model endpoint must be an HTTP base URL")
        self.target = target
        self.runtime = runtime
        self.lock = threading.Lock()
        self.armed = False
        self.blocked = threading.Event()
        self.abandon = threading.Event()
        self.records = []
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_POST(self):
                if self.path != "/v1/embeddings":
                    self.send_error(404)
                    return
                length = int(self.headers.get("Content-Length", "0"))
                require(0 < length <= 2 * 1024 * 1024, "proxy request exceeded its byte budget")
                payload = self.rfile.read(length)
                parsed = json.loads(payload)
                values = parsed.get("input", [])
                values = values if isinstance(values, list) else [values]
                record = {"model": parsed.get("model"), "input_count": len(values),
                          "input_bytes": sum(len(value.encode()) for value in values),
                          "forwarded": False, "abandoned": False}
                with owner.lock:
                    hold, owner.armed = owner.armed, False
                    owner.records.append(record)
                if hold:
                    owner.blocked.set()
                    owner.abandon.wait(timeout=90)
                    # This branch closes a pending client connection, without
                    # making or rewriting a request to the paid upstream model.
                    record["abandoned"] = True
                    self.close_connection = True
                    return
                connection_type = HTTPSConnection if target.scheme == "https" else HTTPConnection
                connection = connection_type(target.hostname, target.port, timeout=120)
                try:
                    record["forwarded"] = True
                    connection.request("POST", target.path.rstrip("/") + "/embeddings", body=payload,
                                       headers={"Content-Type": self.headers.get("Content-Type", "application/json"),
                                                "Authorization": self.headers.get("Authorization", "")})
                    response = connection.getresponse()
                    body = response.read(16 * 1024 * 1024 + 1)
                    require(len(body) <= 16 * 1024 * 1024, "provider response exceeded proxy byte budget")
                    record["status"] = response.status
                    try:
                        result = json.loads(body)
                        record.update(provider_model=result.get("model"), usage=result.get("usage"),
                                      dimensions=[len(row.get("embedding", [])) for row in result.get("data", [])])
                    except ValueError:
                        pass
                    self.send_response(response.status)
                    for key, value in response.getheaders():
                        if key.lower() in {"content-type", "retry-after", "x-request-id", "x-oneapi-request-id"}:
                            self.send_header(key, value)
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                except (BrokenPipeError, ConnectionResetError):
                    record["client_disconnected"] = True
                except Exception as error:
                    record["error_type"] = type(error).__name__
                    self.close_connection = True
                finally:
                    connection.close()

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
        self.thread.start()
        self.endpoint = f"http://127.0.0.1:{self.server.server_port}/v1/"

    def arm(self):
        with self.lock:
            require(not self.armed and not self.blocked.is_set(), "gateway interruption was already armed")
            self.armed = True

    def close(self):
        self.abandon.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)
        (self.runtime / "embedding-proxy.json").write_text(json.dumps(self.records, ensure_ascii=False, indent=2), encoding="utf-8")


def prepare(runner, template):
    configured = tomllib.loads((template / "instance.toml").read_text(encoding="utf-8"))
    models = configured.get("models", {})
    binding = models.get("text_embedding") or models.get("memory_formation")
    require(binding is not None, "template has no model resource credential")
    resource = binding["resource"]
    relative = Path(resource["credential"]["file"])
    require(not relative.is_absolute() and ".." not in relative.parts, "template credential must be an instance relative path")
    credential = (template / relative).resolve(strict=True)
    require(credential.is_relative_to((template / "credentials").resolve()), "template credential escapes credentials/")
    instance = runner.root / "primary"
    runner.cli(instance, "init")
    destination = instance / "credentials" / "embedding-gateway.token"
    descriptor = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as output:
        output.write(credential.read_bytes())
    gateway = DelayedGateway(resource["endpoint"], runner.runtime)
    config_path = instance / "instance.toml"
    config_path.write_text(config_path.read_text(encoding="utf-8") + "\n" + "\n".join([
        "[models.text_embedding]", 'expected_provider_model = "doubao-embedding-vision-251215"',
        "dimension = 2048", "max_batch_size = 32", "max_input_bytes = 32768", "max_calls_per_day = 100",
        'producer_version = "nous.embedding.text.v1"', "[models.text_embedding.resource]",
        'id = "m3-real-text-embedding"', "provider = " + json.dumps(resource.get("provider", "new-api")),
        'protocol = "openai_embeddings"', "endpoint = " + json.dumps(gateway.endpoint),
        'model = "doubao-embedding-vision"', "[models.text_embedding.resource.credential]",
        'file = "credentials/embedding-gateway.token"', "",
    ]), encoding="utf-8")
    try:
        require(runner.cli(instance, "config", "validate")["valid"], "embedding config was invalid")
        require(runner.cli(instance, "model", "check")["text_embedding"]["state"] == "ready", "embedding credential is not locally ready")
    except Exception:
        gateway.close()
        raise
    return instance, gateway


def sql(instance, statement, parameters=()):
    uri = (instance / "data" / "authority.sqlite").resolve().as_uri() + "?mode=ro"
    with contextlib.closing(sqlite3.connect(uri, uri=True, timeout=2)) as database:
        database.row_factory = sqlite3.Row
        return [dict(row) for row in database.execute(statement, parameters)]


def call_count(instance):
    return sql(instance, "SELECT count(*) AS count FROM model_calls")[0]["count"]


def wait_runs(runner, instance, base, receipt, completed_scope=None):
    jobs = list(receipt["result"]["jobs"])
    if not jobs and completed_scope is not None:
        # Startup and memory.index may finish these inputs before explicit sync.
        # Discover their existing scope/space runs without scheduling another call,
        # then exercise the same real HTTP/MCP read paths as newly accepted jobs.
        namespace, subject = completed_scope
        jobs = [row["job_id"] for row in sql(instance, """
            SELECT r.job_id FROM embedding_runs r
            WHERE r.namespace_id=? AND r.subject_id=? AND r.space_id=? AND r.state='succeeded'
              AND EXISTS (SELECT 1 FROM embedding_inputs i WHERE i.job_id=r.job_id AND i.kind IN ('memory','tag'))
            ORDER BY r.created_at,r.job_id
            """, (namespace, subject, receipt["result"]["space"]["id"]))]
    results = []
    for job_id in jobs:
        deadline = time.monotonic() + runner.timeout
        while True:
            run = runner.http(instance, base, "/v1/embeddings/runs/" + job_id)[1]
            if run["state"] == "succeeded":
                results.append(run)
                break
            require(run["state"] not in {"failed", "cancelled", "waiting_for_configuration", "waiting_for_budget"},
                    f"embedding {job_id} stopped in {run['state']}: {run.get('error')}")
            require(time.monotonic() < deadline, f"embedding {job_id} did not finish within {runner.timeout:g}s")
            time.sleep(0.1)
    return results


def hit_ids(result):
    return {hit["memory"]["id"] for hit in result["hits"]}


def exercise(runner, instance, gateway):
    print(json.dumps({"stage": "import_and_sync", "instance": str(instance)}), flush=True)
    base = runner.start(instance)
    namespace = runner.cli(instance, "namespace", "create", "M3 coastal research")["result"]["id"]
    subject = runner.cli(instance, "subject", "create", "--namespace", namespace, "--name", "coastal-observer")["result"]["id"]
    foreign_namespace = runner.cli(instance, "namespace", "create", "M3 separate scope")["result"]["id"]
    foreign_subject = runner.cli(instance, "subject", "create", "--namespace", foreign_namespace, "--name", "separate-observer")["result"]["id"]
    records = [
        {"external_id": "m3-coast", "text": "港湾潮汐站记录：周三由林岚采集海水样本，样本使用蓝色保温箱冷链运输。", "source": "m3:coast", "event_time": {"domain": "utc", "coordinate": 1000}, "tags": ["海洋观测", "冷链运输"]},
        {"external_id": "m3-space", "text": "航天观测站在夜间记录月球光谱，观测员陈洛使用红外望远镜。", "source": "m3:space", "event_time": {"domain": "utc", "coordinate": 2000}, "tags": ["天文观测"]},
        {"external_id": "m3-forest", "text": "山地林场在春天测量树木年轮，护林员顾原检查松林。", "source": "m3:forest", "event_time": {"domain": "utc", "coordinate": 3000}, "tags": ["生态调查"]},
    ]
    mcp = helpers.Mcp(runner, instance, base)
    tools = {tool["name"]: tool for tool in mcp.send("tools/list")["tools"]}
    require({"embedding_sync", "embedding_run", "model_call", "query"} <= tools.keys(), "MCP lacks embedding/query tools")
    schema_text = json.dumps(tools["query"]["inputSchema"])
    require(all(mode in schema_text for mode in ("lexical", "dense", "vcp", "hybrid")), "query mode schema omitted a mode")
    imported = mcp.tool("import_text", {"namespace_id": namespace, "subject_id": subject,
                         "format": "json", "content": json.dumps(records, ensure_ascii=False)})
    require(imported["accepted"] == 3 and imported["rejected"] == 0, "initial tagged records were not imported")
    memory_ids = [item["receipt"]["result"]["memory_id"] for item in imported["items"]]
    coast, space, forest = memory_ids
    foreign_record = dict(records[0], external_id="m3-foreign-coast", source="m3:foreign")
    foreign = runner.http(instance, base, "/v1/imports/text", {"namespace_id": foreign_namespace,
                        "subject_id": foreign_subject, "format": "json", "content": json.dumps([foreign_record], ensure_ascii=False)})[1]
    foreign_memory = foreign["items"][0]["receipt"]["result"]["memory_id"]
    runner.stop(instance)
    subject_token = Path(runner.cli(instance, "token", "create", "--namespace", namespace, "--subject", subject, "--name", "m3-subject-reader")["token_file"])
    namespace_token = Path(runner.cli(instance, "token", "create", "--namespace", namespace, "--name", "m3-namespace-manager")["token_file"])
    base = runner.start(instance)
    mcp = helpers.Mcp(runner, instance, base)
    receipt = runner.cli(instance, "memory", "embed", "--subject", subject, "--operation-id", "m3-initial-sync")
    runs = wait_runs(runner, instance, base, receipt, completed_scope=(namespace, subject))
    require(runs, "initial scope has no successful real memory/tag embedding runs")
    foreign_receipt = mcp.tool("embedding_sync", {"subject_id": foreign_subject, "operation_id": "m3-foreign-sync"})
    foreign_runs = wait_runs(runner, instance, base, foreign_receipt, completed_scope=(foreign_namespace, foreign_subject))
    require(foreign_runs, "foreign scope had no embedded candidate for the isolation check")
    require(mcp.tool("embedding_run", {"job_id": runs[0]["job_id"]})["state"] == "succeeded", "MCP embedding_run differs from HTTP")
    require(any(item["kind"] == "tag" for run in runs for item in run["inputs"]), "tag vectors were not actually embedded")
    runner.report["checks"].update(real_embedding_jobs=True, tag_vectors=True, cli_sync=True, http_run=True, mcp_sync_run=True)

    query_text = "海水采样后的低温保存运输"

    def query(mode="dense", **fields):
        return mcp.tool("query", dict({"subject_id": subject, "text": query_text, "mode": mode, "limit": 10}, **fields))

    print(json.dumps({"stage": "semantic_query_and_cache"}), flush=True)
    dense = runner.cli(instance, "query", "--subject", subject, "--mode", "dense", query_text)
    require(dense["hits"] and dense["hits"][0]["memory"]["id"] == coast, "real dense retrieval failed to rank the coastal fact first")
    require(dense["semantic"]["mode"] == "dense" and dense["semantic"]["embedded_memories"] == 3,
            "dense query lacks complete semantic provenance")
    require(foreign_memory not in hit_ids(dense), "foreign namespace entered dense candidates")
    cache_count = call_count(instance)
    vcp = query("vcp")
    hybrid = query("hybrid")
    require(coast in hit_ids(vcp) and vcp["semantic"]["mode"] == "vcp", "real VCP retrieval omitted the coastal fact")
    require(coast in hit_ids(hybrid) and hybrid["semantic"]["mode"] == "hybrid", "hybrid retrieval omitted the coastal fact")
    require(vcp["semantic"]["diagnostics"] and vcp["semantic"]["provenance"], "VCP diagnostics/provenance are missing")
    require(query()["semantic"]["query_embedding_call_id"] == dense["semantic"]["query_embedding_call_id"], "same query did not reuse its vector")
    require(call_count(instance) == cache_count, "mode changes or repeated queries added paid model calls")
    source = runner.cli(instance, "memory", "source", coast)
    require(source["model_call"] is None and source["observation"]["kind"] == "import", "import source claimed a model producer or actual contact")
    require(source["material"]["details"]["tags"] == records[0]["tags"], "source lost original tags")
    provenance = source["embedding_provenance"]
    require(provenance and any(item["memory_revision"] == source["memory"]["revision"] for item in provenance), "source lacks current embedding provenance")
    model_call_id = provenance[0]["model_call_id"]
    call = runner.cli(instance, "model", "call", model_call_id)
    require(call["state"] == "succeeded" and call["response"]["record"]["provider_model"] == "doubao-embedding-vision-251215", "model call lost the actual provider identity")
    require(call["response"]["record"]["usage"]["prompt_tokens"] > 0, "real embedding call has no usage")
    require(mcp.tool("model_call", {"model_call_id": model_call_id}) == call, "MCP model_call differs from CLI")
    bridge = helpers.Mcp(runner, instance, base, stdio=True)
    try:
        require(bridge.tool("embedding_run", {"job_id": runs[0]["job_id"]})["state"] == "succeeded", "stdio embedding tool failed")
    finally:
        bridge.close()
    runner.report["checks"].update(real_dense=True, real_vcp=True, hybrid=True, cached_query=True,
                                  provider_usage=True, embedding_source=True, mcp_model_call=True, stdio_embedding=True)

    # Reopen with the same producer/space and reuse the existing query vector.
    runner.stop(instance)
    base = runner.start(instance)
    mcp = helpers.Mcp(runner, instance, base)
    reopened = query("vcp")
    require(coast in hit_ids(reopened) and reopened["semantic"]["query_embedding_call_id"] == dense["semantic"]["query_embedding_call_id"], "restart lost query cache or semantic results")
    require(runner.cli(instance, "memory", "source", coast)["embedding_provenance"] == provenance, "restart changed embedding provenance")
    require(call_count(instance) == cache_count, "restart query added a paid model call")
    runner.report["checks"]["semantic_restart_cache"] = True

    print(json.dumps({"stage": "authority_boundaries"}), flush=True)
    mcp.headers["Authorization"] = "Bearer " + subject_token.read_text().strip()
    require(foreign_memory not in hit_ids(query("vcp")), "scoped VCP leaked a foreign memory")
    foreign_call = foreign_runs[0]["model_call_id"]
    for tool, arguments in [
        ("query", {"subject_id": foreign_subject, "text": query_text, "mode": "dense"}),
        ("embedding_run", {"job_id": foreign_runs[0]["job_id"]}),
        ("model_call", {"model_call_id": foreign_call}),
        ("embedding_sync", {"subject_id": foreign_subject, "operation_id": "m3-refused-sync"}),
    ]:
        denied = mcp.send("tools/call", {"name": tool, "arguments": arguments})
        require(denied.get("isError") and denied["structuredContent"]["error"]["code"] == "Forbidden", f"{tool} ignored a changed scoped token on the existing MCP session")
    mcp.headers["Authorization"] = "Bearer " + runner.token(instance)
    source_filtered = query("dense", source="m3:space")
    time_filtered = query("vcp", time={"axis": "event", "range": {"domain": "utc", "start": 1500, "end": 2500}})
    require(hit_ids(source_filtered) == {space}, "source predicate was not a hard vector boundary")
    require(hit_ids(time_filtered) == {space}, "event time predicate was not a hard VCP boundary")
    frozen = mcp.tool("memory_change", {"memory_id": coast, "operation_id": "m3-freeze", "expected_revision": source["memory"]["revision"], "action": "freeze"})["result"]
    require(coast not in hit_ids(query("vcp")), "frozen memory survived final vector authority filtering")
    require(call_count(instance) == cache_count, "scope/source/time/frozen checks did not reuse existing vectors")
    prior = mcp.tool("memory_get", {"memory_id": space})
    revised = mcp.tool("memory_change", {"memory_id": space, "operation_id": "m3-revise", "expected_revision": prior["revision"], "action": "revise",
                       "edit": {"title": "修订后的山地运输", "content": "山地考察队使用黄色货车运输岩石标本，所有物资记录在勘测日志。"}})["result"]
    revised_receipt = mcp.tool("embedding_sync", {"subject_id": subject, "operation_id": "m3-revised-sync"})
    wait_runs(runner, instance, base, revised_receipt)
    updated_query = query("dense")
    require(space in hit_ids(updated_query), "revised memory disappeared from semantic retrieval")
    require(all(hit["memory"]["revision"] == revised["revision"] for hit in updated_query["hits"] if hit["memory"]["id"] == space), "old vector revision bypassed authoritative reread")
    revised_source = mcp.tool("memory_source", {"memory_id": space})
    require(revised_source["material"]["text"] == records[1]["text"], "revision changed immutable import material")
    require(any(item["memory_revision"] == revised["revision"] for item in revised_source["embedding_provenance"]), "revision was not adopted into embedding provenance")
    before_purge = call_count(instance)
    trashed = mcp.tool("memory_change", {"memory_id": coast, "operation_id": "m3-trash", "expected_revision": frozen["revision"], "action": "trash"})["result"]
    mcp.headers["Authorization"] = "Bearer " + namespace_token.read_text().strip()
    purged = mcp.tool("memory_change", {"memory_id": coast, "operation_id": "m3-purge", "expected_revision": trashed["revision"], "action": "purge", "force": True})
    require(purged["result"]["purged"] and coast not in hit_ids(query("vcp")), "purged vector candidate remained visible")
    require(sql(instance, "SELECT count(*) AS count FROM embedding_records WHERE memory_id=?", (coast,))[0]["count"] == 0, "purge retained the memory vector")
    require(call_count(instance) == before_purge, "purge triggered a replacement paid embedding")
    mcp.headers["Authorization"] = "Bearer " + runner.token(instance)
    runner.report["checks"].update(scope=True, per_request_mcp_scope=True, source_boundary=True,
                                  time_boundary=True, frozen_boundary=True, revision_boundary=True, purge_boundary=True)

    print(json.dumps({"stage": "interrupted_embedding_recovery"}), flush=True)
    gateway.arm()
    recovery_record = {"external_id": "m3-recovery", "text": "海洋调查队在黎明重新采集港湾盐度样本，并把标本放入冷藏运输箱。", "source": "m3:recovery", "tags": ["冷链运输"]}
    recovery_import = mcp.tool("import_text", {"namespace_id": namespace, "subject_id": subject,
                              "format": "json", "content": json.dumps([recovery_record], ensure_ascii=False)})
    recovery_memory = recovery_import["items"][0]["receipt"]["result"]["memory_id"]
    interrupted = runner.http(instance, base, "/v1/embeddings/sync", {"subject_id": subject, "operation_id": "m3-interrupted-sync"})[1]
    jobs = interrupted["result"]["jobs"]
    require(jobs and gateway.blocked.wait(timeout=30), "accepted embedding did not reach the controlled interruption window")
    placeholders = ",".join("?" for _ in jobs)
    started = sql(instance, f"SELECT id,job_id,state FROM model_calls WHERE job_id IN ({placeholders}) AND state='started'", jobs)
    require(len(started) == 1, "interrupted embedding lacks exactly one persisted Started call")
    runner.host.kill()  # This Popen belongs to this isolated Runner.
    runner.host.wait(timeout=15)
    gateway.abandon.set()
    base = runner.start(instance)
    mcp = helpers.Mcp(runner, instance, base)
    replayed = runner.http(instance, base, "/v1/embeddings/sync", {"subject_id": subject, "operation_id": "m3-interrupted-sync"})[1]
    require(replayed["result"]["jobs"] == jobs, "recovery changed an accepted sync's durable job identities")
    completed = wait_runs(runner, instance, base, replayed)
    old = mcp.tool("model_call", {"model_call_id": started[0]["id"]})
    require(old["state"] == "uncertain", "host restart failed to mark its interrupted call uncertain")
    for run in completed:
        calls = sql(instance, "SELECT id,state FROM model_calls WHERE job_id=? ORDER BY attempt", (run["job_id"],))
        require(sum(call["state"] == "succeeded" for call in calls) == 1, "fixed embedding job did not adopt exactly one successful call")
        adopted = sql(instance, "SELECT r.source_key FROM embedding_records r JOIN model_calls c ON r.model_call_id=c.id WHERE c.job_id=?", (run["job_id"],))
        require(len(adopted) == len(run["inputs"]) and {row["source_key"] for row in adopted} == {item["source_key"] for item in run["inputs"]}, "recovered embedding inputs were lost or adopted twice")
    require(mcp.tool("memory_source", {"memory_id": recovery_memory})["embedding_provenance"], "recovered vector lacks source provenance")
    remaining = query("vcp")
    require(coast not in hit_ids(remaining) and foreign_memory not in hit_ids(remaining), "recovery resurrected a purged or foreign candidate")
    runner.report["checks"].update(accepted_job_crash=True, interrupted_call_uncertain=True,
                                  same_job_recovery=True, single_adoption=True, recovered_source=True)
    runner.report.update(subject_id=subject, memory_ids=memory_ids, foreign_memory_id=foreign_memory,
                         recovery_memory_id=recovery_memory, space=dense["semantic"]["space_id"],
                         initial_jobs=[run["job_id"] for run in runs], initial_sync_jobs=receipt["result"]["jobs"], interrupted_jobs=jobs,
                         interrupted_call_id=started[0]["id"], final_model_calls=call_count(instance),
                         embedding_provider_requests=sum(record["forwarded"] for record in gateway.records))
    runner.stop(instance)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--instance", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=180)
    arguments = parser.parse_args()
    runner = helpers.Runner(arguments.binary.resolve(strict=True), Path(__file__).resolve().parents[1], arguments.timeout, label="m3-smoke")
    runner.command_timeout = runner.request_timeout = 90
    gateway = None
    try:
        instance, gateway = prepare(runner, arguments.instance.resolve(strict=True))
        exercise(runner, instance, gateway)
    except Exception as error:
        report = runner.save(error)
        raise SystemExit(f"M3 smoke failed: {error}\nReport: {report}") from None
    finally:
        runner.close()
        if gateway:
            gateway.close()
    report = runner.save()
    print(json.dumps({"passed": True, "checks": runner.report["checks"], "report": str(report)}, ensure_ascii=False))


if __name__ == "__main__":
    main()
