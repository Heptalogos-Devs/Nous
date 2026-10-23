"""Run M1 CLI/HTTP/MCP, crash recovery and snapshot restore against a real model.

Usage: python scripts/m1-smoke.py <nous-binary> --instance <configured-instance>
The supplied instance is a read-only configuration/credential template. The script
creates its own instances, executable copy, logs and report under project temp/.
This makes real model requests and uses the configured per-instance call budget.
"""
from __future__ import annotations

import argparse
import contextlib
import json
import os
from pathlib import Path
import queue
import shutil
import sqlite3
import subprocess
import threading
import time
import tomllib
import urllib.error
import urllib.request
import uuid


def require(condition, message):
    if not condition:
        raise AssertionError(message)


class Runner:
    def __init__(self, binary: Path, project: Path, timeout: float, label="m1-smoke"):
        self.run_id = time.strftime("%Y%m%dT%H%M%S") + "-" + uuid.uuid4().hex[:8]
        self.runtime = project / "temp" / "runtime" / label / self.run_id
        self.root = project / "temp" / "instances" / label / self.run_id
        self.runtime.mkdir(parents=True)
        self.root.mkdir(parents=True)
        self.binary = self.runtime / binary.name
        shutil.copy2(binary, self.binary)
        self.flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
        self.timeout = timeout
        self.command_timeout = 40
        self.request_timeout = 20
        self.host = None
        self.children = []
        self.host_number = 0
        self.host_log = None
        self.environment = dict(os.environ)
        for name in ("TEMP", "TMP", "TMPDIR"):
            self.environment[name] = str(self.runtime)
        for name in ("NOUS_INSTANCE", "NOUS_URL", "NOUS_TOKEN_FILE"):
            self.environment.pop(name, None)
        self.report = {"run_id": self.run_id, "runtime": str(self.runtime),
                       "instances": str(self.root), "checks": {}}

    def cli(self, instance: Path, *command: str, allow_failure=False):
        result = subprocess.run([str(self.binary), "--instance", str(instance), *command],
                                capture_output=True, text=True, encoding="utf-8", timeout=self.command_timeout,
                                creationflags=self.flags, env=self.environment, cwd=self.runtime)
        if result.returncode:
            if allow_failure:
                return None
            raise AssertionError(f"CLI {' '.join(command)} failed: {result.stderr.strip()}")
        return json.loads(result.stdout)

    def prepare(self, template: Path) -> Path:
        config_bytes = (template / "instance.toml").read_bytes()
        config = tomllib.loads(config_bytes.decode("utf-8"))
        formation = config.get("models", {}).get("memory_formation")
        require(formation is not None, "template has no memory formation model")
        require(formation.get("max_calls_per_day", 20) >= 3,
                "M1 smoke needs a configured daily budget of at least three model calls")
        instance = self.root / "primary"
        initialized = self.cli(instance, "init")
        fresh_token = Path(initialized["administrator_token_file"]).read_bytes()

        def credential_path(root, reference, exists=False):
            relative = Path(reference)
            require(not relative.is_absolute() and ".." not in relative.parts,
                    "credential reference must be a relative instance path")
            resolved = (root / relative).resolve(strict=exists)
            require(resolved.is_relative_to((root / "credentials").resolve()),
                    "credential reference must stay inside credentials/")
            return resolved

        model_reference = formation["resource"]["credential"]["file"]
        admin_reference = config.get("auth", {}).get("admin_token", {}).get("file", "credentials/admin.token")
        model_target = credential_path(instance, model_reference)
        admin_target = credential_path(instance, admin_reference)
        require(model_target != admin_target, "model and administrator credentials require separate files")
        model_source = credential_path(template, model_reference, exists=True)
        model_target.parent.mkdir(parents=True, exist_ok=True)
        descriptor = os.open(model_target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "wb") as output:
            output.write(model_source.read_bytes())
        admin_target.parent.mkdir(parents=True, exist_ok=True)
        descriptor = os.open(admin_target, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(descriptor, "wb") as output:
            output.write(fresh_token)
        (instance / "instance.toml").write_bytes(config_bytes)
        require(self.cli(instance, "model", "check")["state"] == "ready",
                "isolated instance model configuration is not ready")
        self.model_reference = model_reference
        return instance

    def token(self, instance):
        config = tomllib.loads((instance / "instance.toml").read_text(encoding="utf-8"))
        reference = config.get("auth", {}).get("admin_token", {}).get("file", "credentials/admin.token")
        return (instance / reference).read_text(encoding="utf-8").strip()

    def http(self, instance, base, path, data=None, headers=None):
        request_headers = {"Accept": "application/json, text/event-stream",
                           "Authorization": "Bearer " + self.token(instance)}
        request_headers.update(headers or {})
        encoded = None
        if data is not None:
            request_headers["Content-Type"] = "application/json"
            encoded = json.dumps(data, ensure_ascii=False).encode("utf-8")
        request = urllib.request.Request(base + path, data=encoded, headers=request_headers)
        try:
            response = urllib.request.urlopen(request, timeout=self.request_timeout)
        except urllib.error.HTTPError as error:
            raise AssertionError(f"HTTP {path} returned {error.code}: {error.read().decode('utf-8', 'replace')}") from None
        with response:
            if response.status == 202:
                return dict(response.headers), None
            if "text/event-stream" in response.headers.get("Content-Type", ""):
                for line in response:
                    if line.startswith(b"data:") and line[5:].strip():
                        return dict(response.headers), json.loads(line[5:].strip())
                raise AssertionError("MCP stream ended without a response")
            return dict(response.headers), json.load(response)

    def start(self, instance):
        require(self.host is None or self.host.poll() is not None, "a smoke host is already running")
        if self.host_log:
            self.host_log.close()
        self.host_number += 1
        log_path = self.runtime / f"host-{self.host_number:02}.jsonl"
        self.host_log = log_path.open("w", encoding="utf-8")
        self.host = subprocess.Popen([str(self.binary), "--instance", str(instance), "serve", "--listen", "127.0.0.1:0"],
                                     stdout=self.host_log, stderr=self.host_log, creationflags=self.flags,
                                     env=self.environment, cwd=self.runtime)
        self.children.append(self.host)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            require(self.host.poll() is None, f"smoke host exited; inspect {log_path}")
            try:
                host = json.loads((instance / "run" / "host.json").read_text(encoding="utf-8"))
                if host["pid"] == self.host.pid:
                    base = "http://" + host["listen"]
                    self.http(instance, base, "/v1/status")
                    return base
            except (OSError, ValueError, AssertionError):
                pass
            time.sleep(0.05)
        raise AssertionError(f"smoke host did not become ready; inspect {log_path}")

    def stop(self, instance):
        self.cli(instance, "stop")
        require(self.host.wait(timeout=30) == 0, "host did not stop cleanly")

    def wait_job(self, instance, base, job_id):
        deadline = time.monotonic() + self.timeout
        while time.monotonic() < deadline:
            job = self.http(instance, base, "/v1/jobs/" + job_id)[1]
            if job["status"] == "succeeded":
                return job
            require(job["status"] not in {"failed", "cancelled", "waiting_for_configuration", "waiting_for_budget"},
                    f"formation {job_id} stopped in {job['status']}: {job.get('formation', {}).get('error')}")
            time.sleep(0.15)
        raise AssertionError(f"formation {job_id} exceeded {self.timeout:g} seconds")

    def query(self, instance, subject, memory):
        result = self.cli(instance, "query", "--subject", subject, "--min-commit-seq", str(memory["commit_seq"]), memory["title"])
        require(memory["id"] in {hit["memory"]["id"] for hit in result["hits"]}, "lexical query omitted a formed memory")
        return result

    def interrupt_started(self, instance, observation, subject, operation_id):
        observed = queue.Queue()
        host = self.host

        def watch():
            deadline = time.monotonic() + 30
            try:
                while time.monotonic() < deadline:
                    uri = (instance / "data" / "authority.sqlite").resolve().as_uri() + "?mode=ro"
                    with contextlib.closing(sqlite3.connect(uri, uri=True, timeout=2)) as database:
                        row = database.execute("SELECT c.id,c.job_id,c.state FROM model_calls c JOIN formation_runs f ON f.job_id=c.job_id WHERE f.observation_id=? ORDER BY c.attempt", (observation,)).fetchone()
                        state = database.execute("SELECT state FROM formation_runs WHERE observation_id=?", (observation,)).fetchone()
                    if row:
                        require(row[2] == "started", "model completed before the forced-stop window; repeat the smoke run")
                        host.kill()  # This Popen is always a process created by this Runner.
                        host.wait(timeout=15)
                        observed.put({"call_id": row[0], "job_id": row[1]})
                        return
                    if state:
                        require(state[0] not in {"failed", "waiting_for_configuration", "waiting_for_budget"},
                                f"interrupt formation stopped in {state[0]}")
                    time.sleep(0.005)
                raise AssertionError("no persisted Started model call appeared")
            except Exception as error:
                observed.put(error)

        watcher = threading.Thread(target=watch, daemon=True)
        watcher.start()
        # A completed HTTP reply can race the deliberate host kill. The persisted
        # operation ID is replayed below after restart to recover the same receipt.
        self.cli(instance, "memory", "form", "--subject", subject, "--observation", observation,
                 "--operation-id", operation_id, allow_failure=True)
        outcome = observed.get(timeout=35)
        watcher.join(timeout=2)
        if isinstance(outcome, Exception):
            raise outcome
        return outcome

    def save(self, error=None):
        self.report["passed"] = error is None
        if error:
            self.report["error"] = str(error)
        report_path = self.runtime / "report.json"
        report_path.write_text(json.dumps(self.report, ensure_ascii=False, indent=2), encoding="utf-8")
        return report_path

    def close(self):
        for process in reversed(self.children):
            if process.poll() is None:
                process.kill()
                process.wait(timeout=15)
        if self.host_log:
            self.host_log.close()


class Mcp:
    def __init__(self, runner, instance, base, stdio=False):
        self.runner, self.instance, self.base = runner, instance, base
        self.sequence = 0
        self.headers = {}
        self.bridge = None
        self.log = None
        if stdio:
            self.log = (runner.runtime / "mcp-stdio.jsonl").open("w", encoding="utf-8")
            self.bridge = subprocess.Popen([str(runner.binary), "--instance", str(instance), "--url", base, "mcp", "stdio"],
                                           stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log,
                                           text=True, encoding="utf-8", creationflags=runner.flags,
                                           env=runner.environment, cwd=runner.runtime)
            runner.children.append(self.bridge)
            self.messages = queue.Queue()
            def read():
                try:
                    for line in self.bridge.stdout:
                        self.messages.put(json.loads(line))
                finally:
                    self.messages.put(None)
            threading.Thread(target=read, daemon=True).start()
        initialized = self.send("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                                "clientInfo": {"name": "nous-m1-smoke", "version": "1"}})
        self.protocol = initialized["protocolVersion"]
        self.headers["MCP-Protocol-Version"] = self.protocol
        self.send("notifications/initialized", notification=True)

    def send(self, method, params=None, notification=False):
        self.sequence += 1
        message = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            message["params"] = params
        if not notification:
            message["id"] = self.sequence
        if self.bridge:
            self.bridge.stdin.write(json.dumps(message, ensure_ascii=False) + "\n")
            self.bridge.stdin.flush()
            if notification:
                return None
            while True:
                response = self.messages.get(timeout=30)
                require(response is not None, "stdio MCP bridge closed before its response")
                if response.get("id") == message["id"]:
                    break
        else:
            headers, response = self.runner.http(self.instance, self.base, "/mcp", message, self.headers)
            if method == "initialize":
                for name, value in headers.items():
                    if name.lower() == "mcp-session-id":
                        self.headers["Mcp-Session-Id"] = value
            if notification:
                return None
        require("error" not in response, f"MCP {method} failed: {response.get('error')}")
        return response["result"]

    def tool(self, name, arguments):
        result = self.send("tools/call", {"name": name, "arguments": arguments})
        require(not result.get("isError"), f"MCP tool {name} failed: {result.get('structuredContent')}")
        return result["structuredContent"]

    def close(self):
        if self.bridge:
            self.bridge.stdin.close()
            try:
                self.bridge.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.bridge.kill()
                self.bridge.wait(timeout=10)
            self.log.close()


def exercise(runner: Runner, template: Path):
    instance = runner.prepare(template)
    base = runner.start(instance)
    namespace = runner.cli(instance, "namespace", "create", "M1 live smoke")["result"]["id"]
    subject = runner.cli(instance, "subject", "create", "--namespace", namespace, "--name", "researcher")["result"]["id"]
    text = "北极站的观测点代号为晨星，研究员林岚负责周三的冰层厚度采样；所有样本写入独立的冷链目录。"
    source_file = runner.runtime / "source.txt"
    source_file.write_text(text, encoding="utf-8")
    material = runner.cli(instance, "material", "put", "--namespace", namespace, "--file", str(source_file), "--source", "m1-smoke:arctic")["result"]["id"]
    observation = runner.cli(instance, "observation", "record", "--subject", subject, "--material", material)["result"]["id"]
    mcp = Mcp(runner, instance, base)
    names = {tool["name"] for tool in mcp.send("tools/list")["tools"]}
    require({"memory_form", "memory_get", "memory_history", "memory_source", "query"} <= names, "M1 MCP tools are missing")
    receipt = mcp.tool("memory_form", {"operation_id": "m1-first-formation", "subject_id": subject, "observation_id": observation})
    first_job = receipt["job_ids"][0]
    completed = runner.wait_job(instance, base, first_job)
    memory_id = completed["formation"]["memory_ids"][0]
    runner.report.update(namespace_id=namespace,subject_id=subject,material_id=material,
                         first_job_id=first_job,first_memory_id=memory_id)
    memory = runner.cli(instance, "memory", "get", memory_id)
    result = runner.query(instance, subject, memory)
    provenance = mcp.tool("memory_source", {"memory_id": memory_id})
    require(provenance["material"]["text"] == text and provenance["observation"]["id"] == observation, "formed source trace changed")
    require(provenance["model_call"]["state"] == "succeeded", "source has no successful real model call")
    require(len(mcp.tool("memory_history", {"memory_id": memory_id})) == 1, "fresh memory history has unexpected revisions")
    require(memory_id in {hit["memory"]["id"] for hit in mcp.tool("query", {"subject_id": subject, "text": memory["title"]})["hits"]}, "HTTP MCP query omitted memory")
    bridge = Mcp(runner, instance, base, stdio=True)
    try:
        require(bridge.tool("memory_get", {"memory_id": memory_id})["id"] == memory_id, "stdio memory_get changed identity")
        require(bridge.tool("memory_source", {"memory_id": memory_id})["material"]["material"]["id"] == material, "stdio source trace changed")
        require(memory_id in {hit["memory"]["id"] for hit in bridge.tool("query", {"subject_id": subject, "text": memory["title"]})["hits"]}, "stdio MCP query omitted memory")
    finally:
        bridge.close()
    runner.stop(instance)
    base = runner.start(instance)
    require(runner.cli(instance, "memory", "get", memory_id) == memory, "memory changed after restart")
    runner.query(instance, subject, memory)
    runner.report["checks"].update(real_formation=True, lexical_query=True, source_trace=True, mcp_http=True, mcp_stdio=True, restart=True)

    second_observation = runner.cli(instance, "observation", "record", "--subject", subject, "--material", material)["result"]["id"]
    interrupted = runner.interrupt_started(instance, second_observation, subject, "m1-interrupted-formation")
    runner.report["interrupted"] = interrupted
    base = runner.start(instance)
    replay = runner.cli(instance, "memory", "form", "--subject", subject, "--observation", second_observation, "--operation-id", "m1-interrupted-formation")
    require(replay["job_ids"] == [interrupted["job_id"]], "restart did not preserve the fixed formation job ID")
    recovered = runner.wait_job(instance, base, interrupted["job_id"])
    calls = recovered["model_calls"]
    old_call = next(call for call in calls if call["id"] == interrupted["call_id"])
    require(old_call["state"] == "uncertain", "interrupted Started call was not retained as uncertain")
    require(any(call["state"] == "succeeded" and call["id"] != old_call["id"] for call in calls), "recovery did not save a new successful call")
    recovered_ids = recovered["formation"]["memory_ids"]
    require(len(recovered_ids) == len(set(recovered_ids)) and recovered_ids, "recovery returned duplicate or empty adopted memories")
    runner.stop(instance)
    base = runner.start(instance)
    stable_job = runner.cli(instance, "job", "get", interrupted["job_id"])
    require(stable_job["formation"]["memory_ids"] == recovered_ids and len(stable_job["model_calls"]) == len(calls), "successful recovery was repeated after another restart")
    uri = (instance / "data" / "authority.sqlite").resolve().as_uri() + "?mode=ro"
    with contextlib.closing(sqlite3.connect(uri, uri=True)) as database:
        adopted = database.execute("SELECT count(*) FROM operation_receipts WHERE operation='memory.adopt' AND operation_id=?", (interrupted["job_id"],)).fetchone()[0]
        linked = database.execute("SELECT count(*) FROM formation_memories WHERE job_id=?", (interrupted["job_id"],)).fetchone()[0]
    require(adopted == 1 and linked == len(recovered_ids), "fixed job output was not adopted exactly once")
    runner.report["checks"].update(forced_stop_after_started=True, uncertain_call_retained=True, retry_succeeded=True, adopted_once=True)

    backup = Path(runner.cli(instance, "backup", "create")["backup"])
    old_admin = runner.token(instance)
    runner.stop(instance)
    restored = runner.root / "restored"
    runner.cli(restored, "backup", "restore", str(backup))
    require(runner.token(restored) != old_admin, "restore reused the old administrator credential")
    require(not (restored / runner.model_reference).exists(), "restore exported the model credential")
    base = runner.start(restored)
    require(runner.cli(restored, "memory", "get", memory_id) == memory, "snapshot restore changed memory identity or revision")
    runner.query(restored, subject, memory)
    restored_source = runner.cli(restored, "memory", "source", memory_id)
    require(restored_source == provenance, "snapshot restore changed source or model-call records")
    require(runner.cli(restored, "job", "get", interrupted["job_id"])["formation"]["memory_ids"] == recovered_ids, "snapshot restore changed recovered memory IDs")
    require(runner.cli(restored, "model", "check")["state"] == "waiting_configuration", "restored instance unexpectedly has model credentials")
    runner.stop(restored)
    runner.report["checks"].update(snapshot_restore=True, restored_ids_preserved=True, fresh_administrator=True, model_credentials_excluded=True)
    runner.report.update(namespace_id=namespace, subject_id=subject, material_id=material,
                         first_job_id=first_job, first_memory_id=memory_id, recovery_job_id=interrupted["job_id"],
                         recovered_memory_ids=recovered_ids, recovery_calls=[{"id": call["id"], "state": call["state"], "attempt": call["attempt"]} for call in calls],
                         first_query_coverage=result["coverage"], backup=str(backup), first_model_usage=provenance["model_call"].get("response", {}).get("record", {}).get("usage"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--instance", type=Path, required=True, help="read-only configured model instance template")
    parser.add_argument("--timeout", type=float, default=180, help="seconds to wait for each real formation")
    args = parser.parse_args()
    require(args.timeout > 0, "timeout must be positive")
    runner = Runner(args.binary.resolve(strict=True), Path(__file__).resolve().parents[1], args.timeout)
    try:
        exercise(runner, args.instance.resolve(strict=True))
    except Exception as error:
        report = runner.save(error)
        raise SystemExit(f"M1 smoke failed: {error}\nReport: {report}") from None
    finally:
        runner.close()
    report = runner.save()
    print(json.dumps({"passed": True, "checks": runner.report["checks"], "report": str(report)}, ensure_ascii=False))


if __name__ == "__main__":
    main()
