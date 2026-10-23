"""Shared real-process and MCP transport mechanics for the two smoke entrypoints."""
from __future__ import annotations
import contextlib
import copy
import hashlib
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


def toml_value(value):
    if isinstance(value, dict):
        return "{ " + ", ".join(json.dumps(key) + " = " + toml_value(item) for key, item in value.items()) + " }"
    if isinstance(value, list):
        return "[" + ", ".join(toml_value(item) for item in value) + "]"
    return json.dumps(value, ensure_ascii=False, allow_nan=False)


class Runner:
    def __init__(self, binary, timeout=180, label="smoke"):
        project = Path(__file__).resolve().parents[1]
        self.run_id = time.strftime("%Y%m%dT%H%M%S") + "-" + uuid.uuid4().hex[:8]
        self.runtime = project / "temp/runtime" / label / self.run_id
        self.root = project / "temp/instances" / label / self.run_id
        self.runtime.mkdir(parents=True)
        self.root.mkdir(parents=True)
        self.binary = self.runtime / binary.name
        shutil.copy2(binary, self.binary)
        self.flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
        self.timeout = timeout
        self.command_timeout = self.request_timeout = timeout + 30
        self.host = self.host_log = None
        self.children, self.host_number = [], 0
        self.environment = dict(os.environ)
        self.environment.update(TEMP=str(self.runtime), TMP=str(self.runtime), TMPDIR=str(self.runtime))
        for name in ("NOUS_INSTANCE", "NOUS_URL", "NOUS_TOKEN_FILE"):
            self.environment.pop(name, None)
        self.report = {"run_id": self.run_id, "runtime": str(self.runtime), "instances": str(self.root), "checks": {}}

    def cli(self, instance, *command, token_file=None, allow_failure=False):
        arguments = [str(self.binary), "--instance", str(instance)]
        if token_file:
            arguments += ["--token-file", str(token_file)]
        process = subprocess.run(arguments + list(command), capture_output=True, text=True,
                                 encoding="utf-8", timeout=self.command_timeout,
                                 creationflags=self.flags, env=self.environment, cwd=self.runtime)
        if process.returncode:
            if allow_failure:
                return None
            raise AssertionError(f"CLI {' '.join(command)} failed: {process.stderr.strip()}")
        return json.loads(process.stdout)

    def token(self, instance):
        config = tomllib.loads((instance / "instance.toml").read_text(encoding="utf-8"))
        path = config["auth"]["admin_token"]["file"]
        return (instance / path).read_text(encoding="utf-8").strip()

    def http(self, instance, base, path, data=None, headers=None, token_file=None, expected=200):
        token = token_file.read_text(encoding="utf-8").strip() if token_file else self.token(instance)
        request_headers = {"Accept": "application/json, text/event-stream", "Authorization": "Bearer " + token}
        request_headers.update(headers or {})
        if data is not None:
            request_headers["Content-Type"] = "application/json"
        body = None if data is None else json.dumps(data, ensure_ascii=False).encode("utf-8")
        request = urllib.request.Request(base + path, data=body, headers=request_headers)
        try:
            response = urllib.request.urlopen(request, timeout=self.request_timeout)
        except urllib.error.HTTPError as error:
            with error:
                payload = json.load(error)
                require(error.code == expected, f"HTTP {path} returned {error.code}: {payload}")
                return dict(error.headers), payload
        with response:
            require(response.status in {expected, 202}, f"HTTP {path} returned {response.status}")
            if response.status == 202:
                return dict(response.headers), None
            if "text/event-stream" in response.headers.get("Content-Type", ""):
                for line in response:
                    if line.startswith(b"data:") and line[5:].strip():
                        return dict(response.headers), json.loads(line[5:].strip())
                raise AssertionError("MCP stream ended before its response")
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
            require(self.host.poll() is None, f"host exited; inspect {log_path}")
            try:
                host = json.loads((instance / "run/host.json").read_text(encoding="utf-8"))
                if host["pid"] == self.host.pid:
                    base = "http://" + host["listen"]
                    self.http(instance, base, "/api/status")
                    return base
            except (OSError, ValueError, AssertionError):
                pass
            time.sleep(0.05)
        raise AssertionError(f"host did not become ready; inspect {log_path}")

    def stop(self, instance):
        self.cli(instance, "stop")
        require(self.host.wait(timeout=30) == 0, "host did not stop cleanly")

    def wait_job(self, instance, base, identity, state="succeeded", wait_kind=None):
        deadline = time.monotonic() + self.timeout
        while time.monotonic() < deadline:
            snapshot = self.http(instance, base, "/api/jobs/" + identity)[1]
            job = snapshot["job"]
            if job["state"] in {"succeeded", "failed", "cancelled", "waiting"}:
                self.record("job-" + identity + ".json", snapshot)
            if job["state"] == state and (wait_kind is None or job["wait"]["kind"] == wait_kind):
                return snapshot
            require(job["state"] not in {"failed", "cancelled", "waiting"},
                    f"job {identity} stopped in {job['state']}: {job.get('error')}; inspect its saved job record")
            time.sleep(0.15)
        raise AssertionError(f"job {identity} exceeded {self.timeout:g} seconds")

    def call_count(self, instance):
        database = Path(self.cli(instance, "paths")["database"])
        uri = "file:" + urllib.request.pathname2url(str(database.resolve())) + "?mode=ro"
        with contextlib.closing(sqlite3.connect(uri, uri=True, timeout=2)) as connection:
            return connection.execute("SELECT count(*) FROM model_calls").fetchone()[0]

    def record(self, name, value):
        (self.runtime / name).write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")

    def save(self, error=None):
        self.report["passed"] = error is None
        if error:
            self.report["error"] = str(error)
        self.record("report.json", self.report)
        return self.runtime / "report.json"

    def close(self):
        for process in reversed(self.children):
            if process.poll() is None:
                process.kill()
                process.wait(timeout=15)
        if self.host_log:
            self.host_log.close()


def prepare_template(runner, template):
    source = tomllib.loads((template / "instance.toml").read_text(encoding="utf-8"))
    models = source.get("models", {})
    require(models.get("memory_formation") and models.get("text_embedding"), "template requires chat and text_embedding resources")
    instance = runner.root / "primary"
    runner.cli(instance, "init")
    config = tomllib.loads((instance / "instance.toml").read_text(encoding="utf-8"))
    config["network"] = {key: value for key, value in source.get("network", {}).items()
                         if key in {"proxy", "connect_timeout_seconds", "request_timeout_seconds"}}
    role_fields = {"memory_formation": {"max_input_bytes", "max_output_tokens", "max_calls_per_day", "native_parameters", "prompt"},
                   "structured_formation": {"max_input_bytes", "max_output_tokens", "max_calls_per_day", "native_parameters", "prompt"},
                   "text_embedding": {"expected_provider_model", "dimension", "max_batch_size", "max_input_bytes", "max_calls_per_day"}}
    bindings = copy.deepcopy(models)
    if "structured_formation" not in bindings:
        chat = bindings["memory_formation"]
        bindings["structured_formation"] = {"resource": chat["resource"], "native_parameters": chat.get("native_parameters", {}),
                                             "max_input_bytes": 131072, "max_output_tokens": 8192,
                                             "max_calls_per_day": chat.get("max_calls_per_day", 20)}
    configured = {}
    for role, fields in role_fields.items():
        binding = bindings[role]
        current = {key: copy.deepcopy(value) for key, value in binding.items() if key in fields}
        resource = {key: copy.deepcopy(value) for key, value in binding["resource"].items()
                    if key in {"id", "provider", "protocol", "endpoint", "model", "credential"}}
        relative = Path(resource["credential"]["file"])
        require(not relative.is_absolute() and ".." not in relative.parts, "credential reference must be relative")
        credential = (template / relative).resolve(strict=True)
        require(credential.is_relative_to((template / "credentials").resolve()), "credential escapes template credentials directory")
        target = instance / "credentials" / ("gateway-" + hashlib.sha256(str(relative).encode()).hexdigest()[:12] + ".token")
        if not target.exists():
            descriptor = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(descriptor, "wb") as output:
                output.write(credential.read_bytes())
        resource["credential"] = {"file": target.relative_to(instance).as_posix()}
        current["resource"] = resource
        prompt = current.get("prompt", {}).get("file")
        if prompt:
            relative = Path(prompt)
            require(not relative.is_absolute() and ".." not in relative.parts and relative.parts[0] == "prompts", "prompt must be in prompts/")
            original = (template / relative).resolve(strict=True)
            require(original.is_relative_to((template / "prompts").resolve()), "prompt escapes template prompt directory")
            destination = instance / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(original, destination)
        configured[role] = current
    config["models"] = configured
    text = "\n".join(json.dumps(key) + " = " + toml_value(value) for key, value in config.items()) + "\n"
    (instance / "instance.toml").write_text(text, encoding="utf-8")
    require(runner.cli(instance, "config", "validate")["valid"], "isolated configuration is invalid")
    readiness = runner.cli(instance, "model", "check")
    require(all(readiness[role]["state"] == "ready" for role in role_fields), "isolated model roles are not ready")
    return instance


class Mcp:
    def __init__(self, runner, instance, base, stdio=False):
        self.runner, self.instance, self.base = runner, instance, base
        self.sequence = 0
        self.headers = {}
        self.bridge = None
        self.log = None
        if stdio:
            self.log = (runner.runtime / f"mcp-stdio-{runner.host_number:02}.jsonl").open("w", encoding="utf-8")
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
                                                "clientInfo": {"name": "nous-smoke", "version": "development"}})
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
