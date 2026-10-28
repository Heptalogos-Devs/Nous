"""The CLI uses real HTTP, strict JSON and protected one-time credential files."""

import json
import os
import socket
import subprocess
import sys
import time


def test_cli_literal_query_and_revocable_readonly_token(tmp_path):
    command = [sys.executable, "-c", "from nous.transports.cli import main; main()"]
    instance = tmp_path / "instance"
    with socket.socket() as available:
        available.bind(("127.0.0.1", 0))
        port = available.getsockname()[1]
    connection = ["--instance", str(instance), "--url", f"http://127.0.0.1:{port}"]

    def invoke(*arguments, credential=None, success=True, human=False):
        options = connection.copy()
        if credential is not None:
            options += ["--credential-file", str(credential)]
        if not human:
            options += ["--json"]
        response = subprocess.run(
            [*command, *options, *arguments], capture_output=True, encoding="utf-8", timeout=40
        )
        assert (response.returncode == 0) == success, (response.stdout, response.stderr)
        if human:
            return response.stdout
        result = json.loads(response.stdout)
        assert isinstance(result, dict)
        return result

    initialized = invoke("init")
    assert "credential_file" in initialized
    with (tmp_path / "host.log").open("w", encoding="utf-8") as log:
        host = subprocess.Popen(
            [*command, *connection, "serve", "--port", str(port)], stdout=log, stderr=log
        )
        try:
            deadline = time.monotonic() + 40
            while True:
                status = subprocess.run(
                    [*command, *connection, "--json", "status"],
                    capture_output=True,
                    encoding="utf-8",
                    timeout=10,
                )
                if status.returncode == 0:
                    break
                assert host.poll() is None and time.monotonic() < deadline, (
                    tmp_path / "host.log"
                ).read_text(encoding="utf-8")
                time.sleep(0.1)
            namespace = invoke("namespace", "create", "CLI")["id"]
            subject = invoke("subject", "create", namespace)["id"]
            document = tmp_path / "notes.txt"
            document.write_text(
                "Rollback preserves atomic transactions. 回滚保护原子性。", encoding="utf-8"
            )
            invoke("import", namespace, subject, str(document), "--no-llm", "--vectors", "disabled")
            assert len(invoke("query", namespace, subject, "Rollback")["hits"]) == 1
            rendered = invoke(
                "--locale", "zh-CN", "query", namespace, subject, "Rollback", human=True
            )
            assert "命中" in rendered and "回滚保护原子性" in rendered
            config = tmp_path / "credential.json"
            config.write_text(
                json.dumps(
                    {
                        "name": "Reader",
                        "namespace_ids": [namespace],
                        "subject_ids": [subject],
                        "capabilities": ["read"],
                    }
                ),
                encoding="utf-8",
            )
            token_file = tmp_path / "reader.token"
            created = invoke("credentials", "create", str(config), "--token-out", str(token_file))
            assert "token" not in created
            if os.name != "nt":
                assert token_file.stat().st_mode & 0o777 == 0o600
            assert (
                len(invoke("query", namespace, subject, "Rollback", credential=token_file)["hits"])
                == 1
            )
            denied = invoke("subject", "create", namespace, credential=token_file, success=False)
            assert "forbidden" in denied["error"]["code"].lower()
            invoke("credentials", "revoke", created["record"]["id"])
            revoked = invoke(
                "query", namespace, subject, "Rollback", credential=token_file, success=False
            )
            assert revoked["error"]["code"] == "credential_invalid"
            invalid = invoke("unknown-command", success=False)
            assert invalid["error"]["code"] == "InvalidArgument"
            assert invoke("source", success=False)["error"]["code"] == "InvalidArgument"
            assert invoke("model-calls", namespace, "--subject-id", subject)["count"] == 0
        finally:
            host.terminate()
            try:
                host.wait(timeout=25)
            except subprocess.TimeoutExpired:
                host.kill()
                host.wait(timeout=5)
