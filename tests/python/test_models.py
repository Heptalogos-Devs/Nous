"""Model protocol failures through the real SDK, CLI and persistent result path."""

import asyncio
import json
import os
import sqlite3
import subprocess
import sys
import threading
from collections.abc import Generator
from contextlib import closing, contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

from nous.application.configuration import open_configuration_operations
from nous.runtime.bootstrap import initialize_instance


@contextmanager
def model_server(handler: type[BaseHTTPRequestHandler]) -> Generator[ThreadingHTTPServer]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield server
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def model_cli(instance: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable,
            "-c",
            "from nous.transports.cli import main; main()",
            "--instance",
            str(instance),
            "--json",
            *arguments,
        ],
        env={key: value for key, value in os.environ.items() if not key.startswith("NOUS__")},
        capture_output=True,
        encoding="utf-8",
        timeout=45,
        check=False,
    )


def configure_models(
    instance: Path, server: ThreadingHTTPServer, output_mode: str = "text"
) -> None:
    async def configure() -> None:
        await initialize_instance(instance, environment={})
        async with open_configuration_operations(instance, environment={}) as configuration:
            await configuration.update(
                {
                    "logging.level": "ERROR",
                    "providers.local": {
                        "protocol": "openai-chat-completions",
                        "endpoint": f"http://127.0.0.1:{server.server_port}/v1",
                    },
                    "models.reader": {
                        "provider": "local",
                        "model_name": "local-model",
                        "capabilities": ["text", "stream", "structured_output"],
                        "output_modes": ["text", "native", "tool"],
                    },
                    "models.vectors": {
                        "provider": "local",
                        "model_name": "local-embedding",
                        "capabilities": ["embedding"],
                    },
                    "tasks.read": {
                        "model": "reader",
                        "prompt": "summarize" if output_mode == "text" else "custom-reading",
                        "output_mode": output_mode,
                    },
                    "model_execution.structured_retries": 1,
                    "embedding.spaces.test-space": {
                        "model": "vectors",
                        "deployment_revision": "test-v1",
                        "dimensions": 2,
                        "modalities": ["text"],
                        "normalization": "none",
                        "encoding": {
                            "document_instruction": "",
                            "query_instruction": "",
                            "preprocessing_revision": "test-v1",
                            "input_combination": "text",
                        },
                    },
                }
            )

    asyncio.run(configure())


def persisted_results(instance: Path) -> int:
    with closing(
        sqlite3.connect((instance / "data" / "nous.sqlite3").as_uri() + "?mode=ro", uri=True)
    ) as database:
        return database.execute("select count(*) from processing_results").fetchone()[0]


@pytest.mark.parametrize("complete", [False, True], ids=["early-eof", "finished"])
def test_model_stream_requires_provider_completion_before_adoption(
    tmp_path: Path, complete: bool
) -> None:
    submissions: list[object] = []

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_POST(self) -> None:
            submissions.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Connection", "close")
            self.end_headers()
            base = {
                "id": "stream-response",
                "object": "chat.completion.chunk",
                "created": 1700000000,
                "model": "local-model",
            }
            frames = [
                {
                    **base,
                    "choices": [
                        {
                            "index": 0,
                            "delta": {"role": "assistant", "content": "Visible response."},
                            "finish_reason": None,
                        }
                    ],
                }
            ]
            if complete:
                frames.append(
                    {
                        **base,
                        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                        "usage": {"prompt_tokens": 10, "completion_tokens": 3, "total_tokens": 13},
                    }
                )
            for frame in frames:
                self.wfile.write(("data: " + json.dumps(frame) + "\n\n").encode())
            if complete:
                self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
            self.close_connection = True

        def log_message(self, format: str, *args: object) -> None:
            pass

    instance = tmp_path / "stream-instance"
    with model_server(Handler) as server:
        configure_models(instance, server)
        response = model_cli(
            instance,
            "tools",
            "model",
            "run",
            "--task",
            "read",
            "--text",
            "Streaming source.",
            "--stream",
        )
    events = [json.loads(line) for line in response.stdout.splitlines()]
    assert len(submissions) == 1
    assert any(event["event"] == "delta" for event in events)
    if complete:
        assert response.returncode == 0, response.stderr
        assert events[-1]["event"] == "completed"
        restored = model_cli(instance, "tools", "result", "read", events[-1]["result"])
        assert restored.returncode == 0, restored.stderr
        assert json.loads(restored.stdout)["outputs"][0]["text"] == "Visible response."
        assert persisted_results(instance) == 1
    else:
        assert response.returncode != 0
        assert not any(event["event"] == "completed" for event in events)
        error = events[-1]["error"]
        assert error["details"]["partial_output_delivered"] is True
        assert "finish_reason" in error["details"]["provider_error"]["message"]
        original = model_cli(instance, "tools", "content", "read", error["details"]["artifacts"][0])
        assert original.returncode == 0, original.stderr
        assert json.loads(original.stdout)["text"] == "Streaming source."
        assert persisted_results(instance) == 0


@pytest.mark.parametrize("output_mode", ["native", "tool"])
def test_model_structured_repair_reaches_second_request_and_survives_restart(
    tmp_path: Path, output_mode: str
) -> None:
    submissions: list[dict[str, object]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            submissions.append(request)
            content = json.dumps({"answer": "invalid" if len(submissions) == 1 else 42})
            message: dict[str, object] = {"role": "assistant", "content": content}
            if output_mode == "tool":
                message = {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "id": f"call-{len(submissions)}",
                            "type": "function",
                            "function": {"name": "result", "arguments": content},
                        }
                    ],
                }
            body = json.dumps(
                {
                    "id": f"response-{len(submissions)}",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "local-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": message,
                            "finish_reason": "tool_calls" if output_mode == "tool" else "stop",
                        }
                    ],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 3, "total_tokens": 13},
                }
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format: str, *args: object) -> None:
            pass

    instance = tmp_path / "repair-instance"
    schema = tmp_path / "schema.json"
    schema.write_text(
        json.dumps(
            {
                "type": "object",
                "properties": {"answer": {"type": "integer"}},
                "required": ["answer"],
                "additionalProperties": False,
            }
        ),
        encoding="utf-8",
    )
    with model_server(Handler) as server:
        configure_models(instance, server, output_mode)
        response = model_cli(
            instance,
            "tools",
            "model",
            "run",
            "--task",
            "read",
            "--schema",
            str(schema),
            "--text",
            "The answer is 42.",
        )
    assert response.returncode == 0, (response.stdout, response.stderr)
    assert len(submissions) == 2
    assert submissions[1]["messages"][-1]["role"] == ("tool" if output_mode == "tool" else "user")
    result = json.loads(response.stdout)["results"][0]
    assert result["parameters"]["attempts"] == 2
    assert result["parameters"]["usage"]["input_tokens"] == 20
    restored = model_cli(instance, "tools", "result", "read", result["ref"]["address"], "--details")
    assert restored.returncode == 0, restored.stderr
    assert json.loads(json.loads(restored.stdout)["outputs"][0]["text"]) == {"answer": 42}
    assert persisted_results(instance) == 1


@pytest.mark.parametrize(
    "payload, accepted",
    [
        ({"data": None}, False),
        ({"data": [{"index": 0, "embedding": ["not-a-number", 0.5]}]}, False),
        ({"data": [{"index": 0, "embedding": [0.5, 0.25]}], "usage": []}, False),
        ({"data": [{"index": False, "embedding": [0.5, 0.25]}]}, False),
        ({"data": [{"index": 0, "embedding": [0.5, 0.25]}], "usage": None}, True),
    ],
    ids=[
        "missing-vectors",
        "nonnumeric-vector",
        "malformed-usage",
        "boolean-index",
        "unknown-usage",
    ],
)
def test_embedding_response_validation_and_adoption_survive_restart(
    tmp_path: Path, payload: dict[str, object], accepted: bool
) -> None:
    submissions: list[object] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            submissions.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            body = json.dumps({"object": "list", "model": "local-embedding", **payload}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format: str, *args: object) -> None:
            pass

    instance = tmp_path / "embedding-instance"
    with model_server(Handler) as server:
        configure_models(instance, server)
        response = model_cli(
            instance,
            "tools",
            "embedding",
            "run",
            "--space",
            "test-space",
            "--purpose",
            "document",
            "--text",
            "A vector source.",
        )
    assert len(submissions) == 1
    if accepted:
        assert response.returncode == 0, (response.stdout, response.stderr)
        result = json.loads(response.stdout)
        restored = model_cli(
            instance, "tools", "result", "read", result["ref"]["address"], "--details"
        )
        assert restored.returncode == 0, restored.stderr
        assert json.loads(restored.stdout)["parameters"]["usage"] == {}
        vectors = next(output for output in result["outputs"] if output["name"] == "vectors")
        export = tmp_path / "vectors.npy"
        exported = model_cli(
            instance, "tools", "content", "export", vectors["artifact"], "--output", str(export)
        )
        assert exported.returncode == 0, exported.stderr
        import numpy as np

        assert np.array_equal(
            np.load(export, allow_pickle=False), np.array([[0.5, 0.25]], dtype=np.float32)
        )
        assert persisted_results(instance) == 1
        return
    assert response.returncode != 0
    error = json.loads(response.stdout)["error"]
    assert error["code"] == "embedding.invalid_response", (response.stdout, response.stderr)
    reopened = model_cli(instance, "runtime", "check")
    assert reopened.returncode == 0, reopened.stderr
    assert persisted_results(instance) == 0
