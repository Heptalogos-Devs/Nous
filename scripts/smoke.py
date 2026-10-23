"""Exercise the real CLI, HTTP and MCP against an isolated local instance."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import queue
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    project = Path(__file__).resolve().parents[1]
    parent = project / "temp" / "instances" / "smoke"
    parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=parent) as directory:
        root = Path(directory)
        logs = project / "temp" / "runtime" / "smoke.log"
        logs.parent.mkdir(parents=True, exist_ok=True)
        instance = root / "instance"
        flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0

        def cli(*command: str, url: str | None = None, token_file: Path | None = None):
            arguments = [str(binary), "--instance", str(instance)]
            if url:
                arguments += ["--url", url]
            if token_file:
                arguments += ["--token-file", str(token_file)]
            result = subprocess.run(arguments + list(command), capture_output=True, text=True,
                                    encoding="utf-8", timeout=20, creationflags=flags)
            if result.returncode:
                raise AssertionError(f"CLI {' '.join(command)}: {result.stderr}")
            return json.loads(result.stdout)

        assert cli("init")["state"] == "initialized"
        assert cli("init")["state"] == "already_initialized"
        assert cli("status")["state"] == "stopped"
        assert cli("config", "validate")["valid"]
        admin = (instance / "credentials" / "admin.token").read_text().strip()
        assert cli("config", "show")["sources"]["server.listen"] == "instance"

        def http(base: str, path: str, data=None, token=admin, method=None, headers=None):
            request_headers = {"Accept": "application/json, text/event-stream"}
            if token:
                request_headers["Authorization"] = "Bearer " + token
            if headers:
                request_headers.update(headers)
            if data is not None:
                request_headers["Content-Type"] = "application/json"
                data = json.dumps(data, ensure_ascii=False).encode()
            request = urllib.request.Request(base + path, data=data, headers=request_headers, method=method)
            try:
                response = urllib.request.urlopen(request, timeout=15)
            except urllib.error.HTTPError as error:
                return error.code, dict(error.headers), json.load(error)
            with response:
                if response.status == 202:
                    return response.status, dict(response.headers), None
                if "text/event-stream" in response.headers.get("Content-Type", ""):
                    for line in response:
                        if line.startswith(b"data:"):
                            payload = line[5:].strip()
                            if payload:
                                return response.status, dict(response.headers), json.loads(payload)
                    raise AssertionError("MCP stream ended without a response")
                return response.status, dict(response.headers), json.load(response)

        def start(log):
            process = subprocess.Popen([str(binary), "--instance", str(instance), "serve", "--listen", "127.0.0.1:0"],
                                       stdout=log, stderr=log, creationflags=flags)
            for _ in range(150):
                if process.poll() is not None:
                    raise AssertionError(f"host exited with {process.returncode}; see {logs}")
                manifest = instance / "run" / "host.json"
                try:
                    base = "http://" + json.loads(manifest.read_text())["listen"]
                    if http(base, "/v1/status")[0] == 200:
                        return process, base
                except (OSError, ValueError):
                    pass
                time.sleep(0.1)
            process.kill()
            process.wait()
            raise AssertionError("host did not become ready")

        with logs.open("w", encoding="utf-8") as log:
            host, base = start(log)
            try:
                assert http(base, "/v1/status", token=None)[0] == 401
                competing = subprocess.run([str(binary), "--instance", str(instance), "serve"],
                                           capture_output=True, text=True, timeout=10, creationflags=flags)
                assert competing.returncode != 0 and "already in use" in competing.stderr
                namespace = cli("namespace", "create", "research", "--operation-id", "smoke-ns", url=base)
                status, _, replay = http(base, "/v1/namespaces", {"name": "research", "operation_id": "smoke-ns"})
                assert status == 200 and replay == namespace
                assert http(base, "/v1/namespaces", {"name": "changed", "operation_id": "smoke-ns"})[0] == 409
                ns = namespace["result"]["id"]
                subject = cli("subject", "create", "--namespace", ns, "--name", "reader", url=base)["result"]["id"]
                source = root / "source.txt"
                source.write_text("Autumn leaves turn red. 秋天的枫叶会变红。", encoding="utf-8")
                material = cli("material", "put", "--namespace", ns, "--file", str(source), "--source", "smoke:source", url=base)["result"]["id"]
                hidden = http(base, "/v1/materials", {"operation_id": "hidden", "namespace_id": ns, "text": "private source"})[2]["result"]["id"]
                cli("observation", "record", "--subject", subject, "--material", material, "--operation-id", "contact-1", url=base)
                assert cli("source", "read", material, url=base)["text"] == source.read_text(encoding="utf-8")
                cli("stop", url=base)
                assert host.wait(timeout=15) == 0
                token_info = cli("token", "create", "--namespace", ns, "--subject", subject, "--name", "reader")
                token_file = Path(token_info["token_file"])
                scoped = token_file.read_text().strip()
                host, base = start(log)
                assert cli("source", "read", material, url=base, token_file=token_file)["text"] == source.read_text(encoding="utf-8")
                assert http(base, "/v1/materials/" + hidden, token=scoped)[0] == 403
                assert http(base, "/v1/observations", {"operation_id": "self-grant", "subject_id": subject, "material_id": hidden}, token=scoped)[0] == 403
                assert http(base, "/v1/stop", {}, token=scoped)[0] == 403
                contacts = cli("observation", "list", "--subject", subject, url=base, token_file=token_file)
                assert len(contacts) == 1 and contacts[0]["observed_at"] is None

                # Real Streamable HTTP MCP handshake and scope per request.
                status, headers, initialized = http(base, "/mcp", {
                    "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                        "protocolVersion": "2025-06-18", "capabilities": {},
                        "clientInfo": {"name": "nous-smoke", "version": "1"}}})
                assert status == 200 and "result" in initialized
                session = next(value for key, value in headers.items() if key.lower() == "mcp-session-id")
                protocol = initialized["result"]["protocolVersion"]
                mcp_headers = {"Mcp-Session-Id": session, "MCP-Protocol-Version": protocol}
                http(base, "/mcp", {"jsonrpc": "2.0", "method": "notifications/initialized"}, headers=mcp_headers)
                tools = http(base, "/mcp", {"jsonrpc": "2.0", "id": 2, "method": "tools/list"}, headers=mcp_headers)[2]
                assert "material_get" in {tool["name"] for tool in tools["result"]["tools"]}
                denied = http(base, "/mcp", {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "namespace_list", "arguments": {}}}, token=scoped, headers=mcp_headers)[2]
                assert denied["result"]["isError"] and denied["result"]["structuredContent"]["error"]["code"] == "Forbidden"
                memory = http(base, "/mcp", {"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "material_get", "arguments": {"material_id": material}}}, token=scoped, headers=mcp_headers)[2]
                assert memory["result"]["structuredContent"]["text"] == source.read_text(encoding="utf-8")

                # The stdio bridge is an actual client of the same running service.
                bridge = subprocess.Popen([str(binary), "--instance", str(instance), "--url", base, "mcp", "stdio"],
                                          stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log,
                                          text=True, encoding="utf-8", creationflags=flags)
                messages = queue.Queue()
                threading.Thread(target=lambda: [messages.put(json.loads(line)) for line in bridge.stdout], daemon=True).start()

                def stdio(message):
                    bridge.stdin.write(json.dumps(message) + "\n")
                    bridge.stdin.flush()
                    if "id" in message:
                        response = messages.get(timeout=15)
                        assert response["id"] == message["id"]
                        return response

                try:
                    assert "result" in stdio({"jsonrpc": "2.0", "id": 10, "method": "initialize", "params": {
                        "protocolVersion": protocol, "capabilities": {}, "clientInfo": {"name": "stdio-smoke", "version": "1"}}})
                    stdio({"jsonrpc": "2.0", "method": "notifications/initialized"})
                    response = stdio({"jsonrpc": "2.0", "id": 11, "method": "tools/call", "params": {"name": "status", "arguments": {}}})
                    assert response["result"]["structuredContent"]["storage"]["observations"] == 1
                finally:
                    bridge.stdin.close()
                    try:
                        bridge.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        bridge.kill()
                        bridge.wait()
                cli("stop", url=base)
                assert host.wait(timeout=15) == 0
            finally:
                if host.poll() is None:
                    host.kill()
                    host.wait()
        print(json.dumps({"cli": True, "http": True, "mcp_http": True, "mcp_stdio": True,
                          "scoped_tokens": True, "replay": True, "restart": True, "graceful_stop": True}))


if __name__ == "__main__":
    main()
