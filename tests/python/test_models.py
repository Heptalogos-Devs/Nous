"""Model protocol failures through the real SDK, CLI and persistent result path."""

import asyncio
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import threading
import wave
from collections.abc import Generator
from contextlib import closing, contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import cast

import pytest

from nous.application.configuration import open_configuration_operations
from nous.runtime.bootstrap import initialize_instance


@pytest.mark.parametrize("entry", ["model", "media"])
@pytest.mark.parametrize(
    ("primary_ok", "text_first", "asr_configured"),
    [
        (True, False, True),
        (False, False, True),
        (True, True, False),
        (True, True, True),
    ],
    ids=["native-success", "switch-to-text", "text-without-asr", "text-with-asr-down"],
)
def test_audio_route_prepares_transcription_only_after_actual_switch(
    tmp_path: Path, entry: str, primary_ok: bool, text_first: bool, asr_configured: bool
) -> None:
    if not shutil.which("ffmpeg") or not shutil.which("ffprobe"):
        pytest.skip("Audio preparation needs FFmpeg and ffprobe")
    events: list[str] = []
    submissions: list[dict[str, object]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            body = self.rfile.read(int(self.headers["Content-Length"]))
            if self.path.endswith("/audio/transcriptions"):
                events.append("transcription")
                status = 503 if primary_ok else 200
                value = (
                    {"error": {"message": "transcriber unavailable"}}
                    if primary_ok
                    else {"text": "transcript from prerequisite"}
                )
            else:
                request = json.loads(body)
                submissions.append(request)
                events.append(request["model"])
                status = 503 if request["model"] == "native-audio" and not primary_ok else 200
                value = (
                    {"error": {"message": "native audio unavailable"}}
                    if status == 503
                    else {
                        "id": "route-response",
                        "object": "chat.completion",
                        "created": 1700000000,
                        "model": request["model"],
                        "choices": [
                            {
                                "index": 0,
                                "message": {
                                    "role": "assistant",
                                    "content": "Saved route response.",
                                },
                                "finish_reason": "stop",
                            }
                        ],
                        "usage": {"prompt_tokens": 10, "completion_tokens": 4, "total_tokens": 14},
                    }
                )
            payload = json.dumps(value).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, format: str, *args: object) -> None:
            pass

    audio = tmp_path / "source.wav"
    with wave.open(str(audio), "wb") as output:
        output.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
        output.writeframes(b"\0\0" * 16000)
    instance = tmp_path / "audio-route-instance"
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def configure() -> None:
            async with open_configuration_operations(instance, environment={}) as configuration:
                await configuration.update(
                    {
                        "models.native-audio": {
                            "provider": "local",
                            "model_name": "native-audio",
                            "capabilities": ["text", "audio"],
                            "input_formats": ["text/plain", "audio/wav"],
                        },
                        "models.asr": {
                            "provider": "local",
                            "model_name": "asr",
                            "capabilities": ["audio"],
                            "input_formats": ["audio/wav"],
                        },
                        "tasks.read": {
                            "model": "reader" if text_first else "native-audio",
                            "prompt": "summarize",
                            "fallback": [{"model": "native-audio" if text_first else "reader"}],
                        },
                        "tasks.asr": {"model": "asr", "operation": "transcription"},
                        "media.audio.transcription_task": "asr" if asr_configured else None,
                    }
                )

        asyncio.run(configure())
        preview = model_cli(
            instance,
            "tools",
            "prompt",
            "preview",
            "summarize",
            "--task",
            "read",
            "--input",
            str(audio),
        )
        assert preview.returncode == 0, (preview.stdout, preview.stderr)
        shown = json.loads(preview.stdout)
        if text_first and asr_configured:
            assert shown["status"] == "requires_transcription" and shown["model_id"] == "reader"
        else:
            assert shown["model_id"] == "native-audio"
        assert shown["deferred_routes"] == (
            ["native-audio"] if text_first and asr_configured else [] if text_first else ["reader"]
        )
        assert events == []
        arguments = (
            ["tools", "model", "run", "--task", "read", "--input", str(audio)]
            if entry == "model"
            else [
                "tools",
                "media",
                "analyze",
                str(audio),
                "--strategy",
                "description_only",
                "--description-task",
                "read",
            ]
        )
        response = model_cli(instance, *arguments)
    assert response.returncode == 0, (response.stdout, response.stderr)
    result = json.loads(response.stdout)["results"][0]
    if primary_ok:
        assert events == (
            ["transcription", "transcription", "native-audio"]
            if text_first and asr_configured
            else ["native-audio"]
        )
        assert result["producer"]["model_id"] == "native-audio"
        if text_first and asr_configured:
            rejected = result["request"]["ineligible_routes"]["reader"]
            assert rejected["details"]["preparation_route"] == "reader"
            assert rejected["details"]["status_code"] == 503
        messages = cast(list[dict[str, object]], submissions[0]["messages"])
        parts = [
            part
            for message in messages
            if isinstance(message.get("content"), list)
            for part in cast(list[dict[str, object]], message["content"])
        ]
        assert any(part.get("type") == "input_audio" for part in parts)
    else:
        assert events == ["native-audio", "transcription", "local-model"]
        assert result["producer"]["model_id"] == "reader"
        assert "transcript from prerequisite" in json.dumps(submissions[-1])
        assert any(
            part["representation"] == "transcript"
            for group in result["inputs"]
            for part in group["parts"]
        )
        assert len(result["request"]["attempt_reports"]) == 3


@pytest.mark.parametrize("entry", ["model", "media"])
def test_image_routes_share_selection_with_media_and_local_preview(
    tmp_path: Path, entry: str
) -> None:
    from PIL import Image

    models: list[str] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            models.append(request["model"])
            payload = json.dumps(
                {
                    "id": "vision-response",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": request["model"],
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": "The image is red."},
                            "finish_reason": "stop",
                        }
                    ],
                }
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, format: str, *args: object) -> None:
            pass

    image = tmp_path / "image.png"
    Image.new("RGB", (32, 32), "red").save(image)
    instance = tmp_path / "image-route-instance"
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def configure() -> None:
            async with open_configuration_operations(instance, environment={}) as configuration:
                await configuration.update(
                    {
                        "models.vision": {
                            "provider": "local",
                            "model_name": "vision",
                            "capabilities": ["text", "image"],
                            "input_formats": ["text/plain", "image/png"],
                        },
                        "tasks.read": {
                            "model": "reader",
                            "prompt": "summarize",
                            "fallback": [{"model": "vision"}],
                        },
                    }
                )

        asyncio.run(configure())
        preview = model_cli(
            instance,
            "tools",
            "prompt",
            "preview",
            "summarize",
            "--task",
            "read",
            "--input",
            str(image),
        )
        assert preview.returncode == 0, (preview.stdout, preview.stderr)
        assert json.loads(preview.stdout)["model_id"] == "vision" and models == []
        arguments = (
            ["tools", "model", "run", "--task", "read", "--input", str(image)]
            if entry == "model"
            else [
                "tools",
                "media",
                "analyze",
                str(image),
                "--strategy",
                "description_only",
                "--description-task",
                "read",
            ]
        )
        response = model_cli(instance, *arguments)
        assert response.returncode == 0, (response.stdout, response.stderr)
        result = json.loads(response.stdout)["results"][0]
        assert result["producer"]["model_id"] == "vision"
        assert result["inputs"][0]["parts"][0]["media_type"] == "image/png"
        probe = model_cli(instance, "tools", "model", "probe", "vision", "--input", str(image))
        assert probe.returncode == 0, (probe.stdout, probe.stderr)
    assert models == ["vision", "vision"]


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


def test_media_stages_share_routes_and_reuse_completed_description(tmp_path: Path) -> None:
    from PIL import Image

    calls: list[bool] = []
    structure_ok = False

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            structured = "response_format" in request
            calls.append(structured)
            assert request["model"] == "vision"
            status = 503 if structured and not structure_ok else 200
            content = (
                json.dumps(
                    {"title": "Image", "items": [{"text": "Red image", "source_keys": ["S1"]}]}
                )
                if structured
                else "The image is red."
            )
            if structured:
                assert "DESCRIPTION" in json.dumps(request["messages"])
            body = json.dumps(
                {"error": {"message": "structure unavailable"}}
                if status == 503
                else {
                    "id": "media-stage",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "vision",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": content},
                            "finish_reason": "stop",
                        }
                    ],
                }
            ).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format: str, *args: object) -> None:
            pass

    image = tmp_path / "two-stages.png"
    Image.new("RGB", (32, 32), "red").save(image)
    instance = tmp_path / "media-stages-instance"
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def configure() -> None:
            async with open_configuration_operations(instance, environment={}) as configuration:
                await configuration.update(
                    {
                        "models.vision": {
                            "provider": "local",
                            "model_name": "vision",
                            "capabilities": ["text", "image", "structured_output"],
                            "input_formats": ["text/plain", "image/png"],
                            "output_modes": ["text", "native"],
                        },
                        "tasks.describe": {
                            "model": "reader",
                            "prompt": "describe-media",
                            "fallback": [{"model": "vision"}],
                        },
                        "tasks.structure": {
                            "model": "reader",
                            "prompt": "extract-information",
                            "output_mode": "native",
                            "fallback": [{"model": "vision"}],
                        },
                    }
                )

        asyncio.run(configure())
        added = model_cli(instance, "tools", "content", "add", str(image))
        assert added.returncode == 0, added.stdout
        artifact = json.loads(added.stdout)["ref"]["address"]
        command = [
            "tools",
            "media",
            "analyze",
            artifact,
            "--strategy",
            "describe_then_structure",
            "--description-task",
            "describe",
            "--task",
            "structure",
        ]
        failed = model_cli(instance, *command)
        assert failed.returncode != 0
        error = json.loads(failed.stdout)["error"]
        assert error["details"]["failed_stage"] == "structure"
        completed = [
            ref for ref in error["details"]["completed_result_refs"] if ref.startswith("res:")
        ]
        assert len(completed) == 1
        structure_ok = True
        recovered = model_cli(instance, *command, "--description-result", completed[0])
        assert recovered.returncode == 0, (recovered.stdout, recovered.stderr)
    outcome = json.loads(recovered.stdout)
    assert outcome["descriptions"] == completed
    assert calls == [False, True, True, True]
    result = outcome["results"][0]
    assert result["producer"]["model_id"] == "vision"
    assert {group["key"] for group in result["inputs"]} == {"S1", "DESCRIPTION"}
    assert result["coverage"] == outcome["coverage"]["actual"][0]


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


@pytest.mark.parametrize("streaming", [False, True], ids=["response", "stream"])
def test_same_model_preset_fallback_is_prepared_and_persisted(
    tmp_path: Path, streaming: bool
) -> None:
    submissions: list[dict[str, object]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            submissions.append(request)
            status = 503 if len(submissions) == 1 else 200
            content_type = "application/json"
            base = {
                "id": "preset-response",
                "created": 1700000000,
                "model": request["model"],
            }
            if status == 503:
                payload = json.dumps({"error": {"message": "Primary unavailable"}}).encode()
            elif request.get("stream"):
                content_type = "text/event-stream"
                frames = [
                    {
                        **base,
                        "object": "chat.completion.chunk",
                        "choices": [
                            {
                                "index": 0,
                                "delta": {"role": "assistant", "content": "Backup response."},
                                "finish_reason": None,
                            }
                        ],
                    },
                    {
                        **base,
                        "object": "chat.completion.chunk",
                        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                    },
                ]
                payload = (
                    "".join("data: " + json.dumps(frame) + "\n\n" for frame in frames)
                    + "data: [DONE]\n\n"
                ).encode()
            else:
                payload = json.dumps(
                    {
                        **base,
                        "object": "chat.completion",
                        "choices": [
                            {
                                "index": 0,
                                "message": {"role": "assistant", "content": "Backup response."},
                                "finish_reason": "stop",
                            }
                        ],
                    }
                ).encode()
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, format: str, *args: object) -> None:
            pass

    instance = tmp_path / "preset-fallback-instance"
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def configure() -> None:
            async with open_configuration_operations(instance, environment={}) as configuration:
                await configuration.update(
                    {
                        "presets.primary": {"temperature": 0.1, "max_output_tokens": 64},
                        "presets.backup": {"temperature": 0.7, "max_output_tokens": 128},
                        "tasks.read.preset": "primary",
                        "tasks.read.fallback": [{"model": "reader", "preset": "backup"}],
                    }
                )

        asyncio.run(configure())
        response = model_cli(
            instance,
            "tools",
            "model",
            "run",
            "--task",
            "read",
            "--text",
            "Use the backup preset if the primary request fails.",
            *(["--stream"] if streaming else []),
        )
    assert response.returncode == 0, (response.stdout, response.stderr)
    assert [request["model"] for request in submissions] == ["local-model", "local-model"]
    assert [request["temperature"] for request in submissions] == [0.1, 0.7]
    assert [request["max_completion_tokens"] for request in submissions] == [64, 128]
    if streaming:
        events = [json.loads(line) for line in response.stdout.splitlines()]
        assert any(event["event"] == "delta" for event in events)
        assert events[-1]["event"] == "completed"
        reference = events[-1]["result"]
    else:
        reference = json.loads(response.stdout)["results"][0]["ref"]["address"]
    restored = model_cli(instance, "tools", "result", "read", reference, "--details")
    assert restored.returncode == 0, restored.stderr
    result = json.loads(restored.stdout)
    assert result["outputs"][0]["text"] == "Backup response."
    assert result["parameters"]["attempts"] == 2
    assert result["request"]["max_output_tokens"] == 128
    assert result["request"]["prepared_attempt"]["parameters"]["temperature"] == 0.7
    assert result["request"]["prepared_attempt"]["max_output_tokens"] == 128
    assert result["request"]["prepared_attempt"]["attempt_number"] == 2
    assert [report["outcome"] for report in result["request"]["attempt_reports"]] == [
        "retry",
        "success",
    ]
    assert persisted_results(instance) == 1


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
            frames: list[dict[str, object]] = [
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


@pytest.mark.parametrize("fallback", [False, True], ids=["repair", "repair-then-fallback"])
@pytest.mark.parametrize("output_mode", ["native", "tool"])
def test_model_structured_repair_and_fallback_survive_restart(
    tmp_path: Path, output_mode: str, fallback: bool
) -> None:
    submissions: list[dict[str, object]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            submissions.append(request)
            if fallback and len(submissions) == 2:
                body = json.dumps({"error": {"message": "Repair request unavailable"}}).encode()
                self.send_response(503)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
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
        if fallback:

            async def configure() -> None:
                async with open_configuration_operations(instance, environment={}) as configuration:
                    await configuration.update(
                        {
                            "model_execution.max_attempts": 3,
                            "presets.backup": {"temperature": 0.7},
                            "tasks.read.fallback": [{"model": "reader", "preset": "backup"}],
                        }
                    )

            asyncio.run(configure())
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
    assert len(submissions) == (3 if fallback else 2)
    messages = cast(list[dict[str, object]], submissions[1]["messages"])
    assert messages[-1]["role"] == ("tool" if output_mode == "tool" else "user")
    if fallback:
        assert submissions[-1]["temperature"] == 0.7
        assert submissions[-1]["messages"] == submissions[0]["messages"]
    result = json.loads(response.stdout)["results"][0]
    assert result["parameters"]["attempts"] == len(submissions)
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
            instance, "tools", "result", "read", result["results"][0]["address"], "--details"
        )
        assert restored.returncode == 0, restored.stderr
        assert json.loads(restored.stdout)["parameters"]["usage"] == {}
        vectors = result["rows"][0]["artifact"]["address"]
        export = tmp_path / "vectors.npy"
        exported = model_cli(
            instance, "tools", "content", "export", vectors, "--output", str(export)
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
