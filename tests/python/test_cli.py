"""The CLI uses real HTTP, strict JSON and protected one-time credential files."""

import asyncio
import json
import os
import socket
import subprocess
import sys
import time

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client
from nous_wave import NousClient
from opentelemetry.instrumentation.utils import suppress_http_instrumentation


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

            async def streaming_request_isolation():
                async with NousClient(
                    f"http://127.0.0.1:{port}",
                    credential_file=instance / "credentials/admin.token",
                ) as api:
                    observed = []

                    async def capture(response):
                        observed.append(
                            (
                                response.request.headers.get("traceparent"),
                                response.headers["x-nous-trace-id"],
                            )
                        )

                    api.transport.http.event_hooks = {"response": [capture]}
                    chunk = b"No model stream bytes." * 4096

                    async def chunks():
                        for _ in range(4):
                            yield chunk
                            await asyncio.sleep(0)

                    with suppress_http_instrumentation():
                        await api.upload_stream(
                            namespace,
                            chunks(),
                            byte_len=4 * len(chunk),
                            owner_subject_id=subject,
                            media_type="text/plain",
                        )
                        await api.request("GET", "/api/status")
                        await api.request("GET", "/api/status")
                        assert all(parent is None for parent, _ in observed)
                        assert len({identity for _, identity in observed}) == 3
                        headers = await api.transport.headers()
                        headers["traceparent"] = "00-" + "a" * 32 + "-" + "b" * 16 + "-01"
                        response = await api.transport.http.get("/api/status", headers=headers)
                        assert response.headers["x-nous-trace-id"] == "a" * 32

            asyncio.run(streaming_request_isolation())
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
            identity = invoke("credentials", "whoami", credential=token_file)
            assert identity["credential_id"] == created["record"]["id"]
            assert invoke("credentials", "history", created["record"]["id"])["count"] == 1
            rotation = tmp_path / "rotation.json"
            rotation.write_text(
                json.dumps(
                    {"operation_id": "cli-rotate", "expected_revision": 1, "reason": "CLI rollover"}
                ),
                encoding="utf-8",
            )
            invoke(
                "credentials",
                "rotate",
                created["record"]["id"],
                str(rotation),
                "--token-out",
                str(token_file),
                success=False,
            )
            assert invoke("credentials", "read", created["record"]["id"])["revision"] == 1
            replacement = tmp_path / "replacement.token"
            rotated = invoke(
                "credentials",
                "rotate",
                created["record"]["id"],
                str(rotation),
                "--token-out",
                str(replacement),
            )
            assert "token" not in rotated and rotated["record"]["revision"] == 2
            rejected = invoke(
                "query", namespace, subject, "Rollback", credential=token_file, success=False
            )
            assert rejected["error"]["code"] == "credential_invalid"
            replay_target = tmp_path / "replay.token"
            replay = invoke(
                "credentials",
                "rotate",
                created["record"]["id"],
                str(rotation),
                "--token-out",
                str(replay_target),
            )
            assert replay["token"] is None and not replay_target.exists()
            token_file = replacement
            assert invoke("credentials", "history", created["record"]["id"])["count"] == 2
            assert (
                len(invoke("query", namespace, subject, "Rollback", credential=token_file)["hits"])
                == 1
            )
            denied = invoke("subject", "create", namespace, credential=token_file, success=False)
            assert "forbidden" in denied["error"]["code"].lower()

            async def mcp_protocol():
                for readonly in (False, True):
                    parameters = StdioServerParameters(
                        command=command[0],
                        args=[
                            *command[1:],
                            *connection,
                            "mcp",
                            "--namespace-id",
                            namespace,
                            "--subject-id",
                            subject,
                            "--credential-file",
                            str(token_file if readonly else instance / "credentials/admin.token"),
                        ],
                    )
                    with (tmp_path / "mcp.log").open("a", encoding="utf-8") as diagnostics:
                        async with stdio_client(parameters, errlog=diagnostics) as (reader, writer):
                            async with ClientSession(
                                reader, writer, read_timeout_seconds=15
                            ) as session:
                                initialized = await session.initialize()
                                assert initialized.server_info.name == "Nous Wave"
                                tools = await session.list_tools()
                                assert {"manage_sources", "list_jobs", "query_cognition"} <= {
                                    tool.name for tool in tools.tools
                                }
                                arguments = {
                                    "command": {
                                        "inputs": [{"text": "MCP protocol durability."}],
                                        "llm_enabled": False,
                                        "vectors": "disabled",
                                    }
                                }
                                admitted = await session.call_tool("admit_documents", arguments)
                                assert admitted.is_error == readonly
                                found = await session.call_tool(
                                    "query_cognition", {"command": {"text": "protocol"}}
                                )
                                assert not found.is_error
                                assert len(found.structured_content["hits"]) == 1
                                wrong = await session.call_tool(
                                    "manage_sources",
                                    {"action": "list", "command": {"subject_id": "different"}},
                                )
                                assert wrong.is_error
                                manual = await session.read_resource("nous://query/manual")
                                assert manual.contents

            asyncio.run(mcp_protocol())
            assert len(invoke("query", namespace, subject, "protocol")["hits"]) == 1
            assert invoke("jobs", "page", namespace, "--subject-id", subject)["items"] == []
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
