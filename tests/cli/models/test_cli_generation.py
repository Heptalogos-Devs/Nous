"""The local runtime's user-visible path and its persistent contracts."""

import asyncio
import json
import subprocess
import sys
from http.server import BaseHTTPRequestHandler
from pathlib import Path

from nous.application.configuration import open_configuration_operations
from nous.runtime.bootstrap import initialize_instance
from tests.integration.shared.models import model_server


def test_model_cli_fallback_and_structured_output(tmp_path: Path) -> None:
    received_headers: list[dict[str, str]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            received_headers.append(dict(self.headers))
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            content = '["red", "blue"]' if "response_format" in request else "A saved answer."
            body = json.dumps(
                {
                    "id": "response-1",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "test-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": content},
                            "finish_reason": "stop",
                        }
                    ],
                    "usage": {"prompt_tokens": 12, "completion_tokens": 4, "total_tokens": 16},
                }
            ).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format: str, *args: object) -> None:
            pass

    instance = tmp_path / "fallback"

    async def setup() -> None:
        await initialize_instance(instance, environment={})
        async with open_configuration_operations(instance, environment={}) as config:
            await config.update(
                {
                    "providers.dead": {
                        "protocol": "openai-chat-completions",
                        "endpoint": "http://127.0.0.1:9/v1",
                        "network": {"connect_timeout_seconds": 1},
                    },
                    "providers.local": {
                        "protocol": "openai-chat-completions",
                        "endpoint": f"http://127.0.0.1:{server.server_port}/v1",
                    },
                    "models.first": {
                        "provider": "dead",
                        "model_name": "test-model",
                        "capabilities": ["text"],
                    },
                    "models.second": {
                        "provider": "local",
                        "model_name": "test-model",
                        "capabilities": ["text", "structured_output"],
                        "output_modes": ["text", "native"],
                    },
                    "tasks.summary": {
                        "model": "first",
                        "prompt": "summarize",
                        "fallback": [{"model": "second"}],
                    },
                    "tasks.custom": {
                        "model": "second",
                        "prompt": "custom-reading",
                        "output_mode": "native",
                    },
                }
            )

    with model_server(Handler) as server:
        asyncio.run(setup())
        command = [
            sys.executable,
            "-c",
            "from nous.transports.cli import main; main()",
            "--instance",
            str(instance),
            "--json",
        ]
        generated = subprocess.run(
            [
                *command,
                "tools",
                "model",
                "run",
                "--task",
                "summary",
                "--text",
                "Source text.",
                "--show-request",
            ],
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=45,
            check=True,
        )
        result = json.loads(generated.stdout)["results"][0]
        assert result["parameters"]["attempts"] == 2
        assert '"prompt_id": "summarize"' in generated.stderr
        assert result["producer"]["model_id"] == "second"
        assert result["outputs"][0]["view"]["body_text"] == "A saved answer."
        assert len(received_headers) == 1
        assert not any(name.lower() == "authorization" for name in received_headers[0])
        reopened = subprocess.run(
            [*command, "tools", "result", "read", result["ref"]["identifier"], "--sources"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=45,
            check=True,
        )
        restored = json.loads(reopened.stdout)
        assert restored["outputs"][0]["view"]["body_text"] == "A saved answer."
        assert restored["inputs"][0]["parts"][0]["content"]["artifact"]["identifier"].startswith(
            "art("
        )

        schema = tmp_path / "array-schema.json"
        schema.write_text(
            json.dumps({"type": "array", "items": {"type": "string"}}), encoding="utf-8"
        )
        structured = subprocess.run(
            [
                *command,
                "tools",
                "model",
                "run",
                "--task",
                "custom",
                "--schema",
                str(schema),
                "--text",
                "Two colors.",
            ],
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=45,
            check=True,
        )
        output = json.loads(structured.stdout)["results"][0]["outputs"][0]
        assert output["view"]["media_type"] == "application/json"
        assert json.loads(output["view"]["body_text"]) == ["red", "blue"]
