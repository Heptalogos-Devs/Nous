"""Media route selection through the SDK and CLI."""

import asyncio
import json
import shutil
import wave
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from typing import cast

import pytest
from model_support import configure_models, model_cli, model_server

from nous.application.configuration import open_configuration_operations


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
        actual_text = "\n".join(str(part.get("text", "")) for part in parts)
        assert "源音频时长约 1 秒。" in actual_text
        assert "本次提供源文件第 0–1 秒的音频。" in actual_text
    else:
        assert events == ["native-audio", "transcription", "local-model"]
        assert result["producer"]["model_id"] == "reader"
        assert "transcript from prerequisite" in json.dumps(submissions[-1])
        actual_messages = json.dumps(submissions[-1]["messages"], ensure_ascii=False)
        assert "源音频时长约 1 秒。" in actual_messages
        assert "本次提供源文件第 0–1 秒的转写文本。" in actual_messages
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
        artifact = json.loads(added.stdout)["ref"]["identifier"]
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
            ref for ref in error["details"]["completed_result_refs"] if ref.startswith("res(")
        ]
        completed = [
            ref
            for ref in completed
            if json.loads(model_cli(instance, "tools", "result", "read", ref).stdout)["operation"]
            == "describe"
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


def test_media_batch_recovery_reuses_saved_responses_after_reopen(tmp_path: Path) -> None:
    from unittest.mock import patch

    from PIL import Image

    from nous.common.operations import exception_context
    from nous.content.contracts import ResultRef
    from nous.models.generation import ProducedTask
    from nous.runtime.bootstrap import open_instance

    calls: list[object] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            calls.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            payload = json.dumps(
                {
                    "id": "media-batch",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "local-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": "Page description."},
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

    pdf = tmp_path / "pages.pdf"
    page = Image.new("RGB", (20, 20), "red")
    page.save(pdf, save_all=True, append_images=[Image.new("RGB", (20, 20), "blue")])
    instance = tmp_path / "media-recovery"
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def scenario() -> None:
            async with open_configuration_operations(instance, environment={}) as configuration:
                await configuration.update(
                    {
                        "models.reader.capabilities": ["text", "image"],
                        "models.reader.input_formats": ["text/plain", "image/png"],
                        "media.pdf.max_pages_per_request": 1,
                    }
                )
            async with open_instance(instance, environment={}) as runtime:
                original = runtime.generation.prepare_adoption
                count = 0

                async def fail_second(produced: ProducedTask, *, producer_key: str | None = None):
                    nonlocal count
                    count += 1
                    if count == 2:
                        raise OSError("injected adoption failure")
                    return await original(produced, producer_key=producer_key)

                with patch.object(runtime.generation, "prepare_adoption", side_effect=fail_second):
                    with pytest.raises(OSError) as failure:
                        await runtime.media.analyze(
                            pdf,
                            "description_only",
                            runtime.configuration.snapshot,
                            description_task="read",
                        )
                ref = ResultRef.model_validate(
                    exception_context(failure.value)["media_request_ref"]
                )
                assert len(calls) == 2
            async with open_instance(instance, environment={}) as runtime:
                with patch.object(
                    runtime.generation,
                    "load_language_task",
                    side_effect=AssertionError("provider lookup"),
                ):
                    recovered = await runtime.media.recover(ref, runtime.configuration.snapshot)
                    repeated = await runtime.media.recover(ref, runtime.configuration.snapshot)
                assert [result.ref for result in recovered.batches] == [
                    result.ref for result in repeated.batches
                ]
                assert len(recovered.batches) == 2
                assert len(calls) == 2

        asyncio.run(scenario())


@pytest.mark.parametrize("entry", ["model", "media"])
@pytest.mark.parametrize("primary_ok", [True, False], ids=["native-success", "fallback-needs-asr"])
def test_invalid_transcription_does_not_block_native_audio(
    tmp_path: Path, entry: str, primary_ok: bool
) -> None:
    """A missing backup transcription task is reported only when that route is used."""
    if not shutil.which("ffmpeg") or not shutil.which("ffprobe"):
        pytest.skip("Audio preparation needs FFmpeg and ffprobe")
    events: list[str] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            body = self.rfile.read(int(self.headers["Content-Length"]))
            if self.path.endswith("/audio/transcriptions"):
                events.append("transcription")
                status, value = 200, {"text": "Unexpected transcription"}
            else:
                request = json.loads(body)
                events.append(request["model"])
                status = 200 if primary_ok else 503
                value = (
                    {
                        "id": "native",
                        "object": "chat.completion",
                        "created": 1700000000,
                        "model": request["model"],
                        "choices": [
                            {
                                "index": 0,
                                "message": {"role": "assistant", "content": "Audio received."},
                                "finish_reason": "stop",
                            }
                        ],
                    }
                    if primary_ok
                    else {"error": {"message": "native route unavailable"}}
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
    instance = tmp_path / "invalid-transcription"
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def configure() -> None:
            async with open_configuration_operations(instance, environment={}) as config:
                await config.update(
                    {
                        "models.native-audio": {
                            "provider": "local",
                            "model_name": "native-audio",
                            "capabilities": ["text", "audio"],
                            "input_formats": ["text/plain", "audio/wav"],
                        },
                        "tasks.read": {
                            "model": "native-audio",
                            "prompt": "summarize",
                            "fallback": [{"model": "reader"}],
                        },
                        "media.audio.transcription_task": "missing-asr",
                    }
                )

        asyncio.run(configure())
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
                "--from",
                "0.25",
                "--to",
                "0.75",
            ]
        )
        response = model_cli(instance, *arguments)
        data = json.loads(response.stdout)
        assert events == ["native-audio"]
        if primary_ok:
            assert response.returncode == 0, (response.stdout, response.stderr)
            context = data["results"][0]["inputs"][0]["context"]
            assert context["kind"] == "audio"
            selected = context["selections"][0]
            assert (selected["start_ms"], selected["end_ms"]) == (
                (250, 750) if entry == "media" else (0, 1000)
            )
        else:
            assert response.returncode != 0
            error = data["error"]
            assert error["code"] == "model.no_eligible_route"
            unavailable = error["details"]["routes"]["reader"]
            assert unavailable["code"] == "media.audio_capability_missing"
            assert unavailable["details"]["transcription_task"] == "missing-asr"
            assert (
                unavailable["details"]["configuration_error"]["code"] == "model.task_unconfigured"
            )
            completed = error["details"]["completed_result_refs"]
            assert completed
            results = [
                json.loads(model_cli(instance, "tools", "result", "read", ref).stdout)
                for ref in completed
                if ref.startswith("res(")
            ]
            assert any(result["operation"] == "media.prepare.audio" for result in results)
            if entry == "media":
                from nous.common.errors import NousError
                from nous.content.contracts import SelectedInputs
                from nous.models.generation import FrozenLanguageTask, GenerationRequest
                from nous.runtime.bootstrap import open_instance

                async def capture_plan() -> tuple[FrozenLanguageTask, SelectedInputs]:
                    async with open_instance(instance, environment={}) as runtime:
                        snapshot = runtime.configuration.snapshot
                        selected = await runtime.generation.accept_inputs(audio, snapshot)
                        frozen = await runtime.generation.freeze_language_task(
                            "read", snapshot, inputs=selected
                        )
                        return frozen, selected

                frozen, selected = asyncio.run(capture_plan())

                async def configure_transcription() -> None:
                    async with open_configuration_operations(instance, environment={}) as config:
                        await config.update(
                            {
                                "models.asr": {
                                    "provider": "local",
                                    "model_name": "asr",
                                    "capabilities": ["audio"],
                                    "input_formats": ["audio/wav"],
                                },
                                "tasks.missing-asr": {"model": "asr", "operation": "transcription"},
                            }
                        )

                asyncio.run(configure_transcription())

                async def retry_plan() -> None:
                    async with open_instance(instance, environment={}) as runtime:
                        with pytest.raises(NousError) as failed:
                            await runtime.generation.produce(
                                GenerationRequest(frozen, selected),
                                computation_key="accepted-plan",
                                snapshot=runtime.configuration.snapshot,
                            )
                        routes = failed.value.details["routes"]
                        assert isinstance(routes, dict)
                        route = routes["reader"]
                        assert isinstance(route, dict)
                        details = route["details"]
                        assert isinstance(details, dict)
                        issue = details["configuration_error"]
                        assert isinstance(issue, dict)
                        assert issue["code"] == "model.task_unconfigured"

                asyncio.run(retry_plan())
                assert events == ["native-audio", "native-audio"]

                # A once-valid frozen transcription binding is checked at fallback,
                # not while the original audio-capable route succeeds.
                frozen, selected = asyncio.run(capture_plan())

                async def change_transcription_binding() -> None:
                    async with open_configuration_operations(instance, environment={}) as config:
                        await config.update({"models.asr.model_name": "changed-asr"})

                asyncio.run(change_transcription_binding())
                primary_ok = True

                async def use_native_route() -> None:
                    async with open_instance(instance, environment={}) as runtime:
                        result = await runtime.generation.produce(
                            GenerationRequest(frozen, selected),
                            computation_key="native-with-stale-asr",
                            snapshot=runtime.configuration.snapshot,
                        )
                        assert result.producer["model_id"] == "native-audio"

                asyncio.run(use_native_route())
                primary_ok = False

                async def use_changed_prerequisite() -> None:
                    async with open_instance(instance, environment={}) as runtime:
                        with pytest.raises(NousError) as failed:
                            await runtime.generation.produce(
                                GenerationRequest(frozen, selected),
                                computation_key="fallback-with-stale-asr",
                                snapshot=runtime.configuration.snapshot,
                            )
                        routes = failed.value.details["routes"]
                        assert isinstance(routes, dict)
                        route = routes["reader"]
                        assert isinstance(route, dict)
                        details = route["details"]
                        assert isinstance(details, dict)
                        issue = details["configuration_error"]
                        assert isinstance(issue, dict)
                        assert issue["code"] == "model.resource_binding"

                asyncio.run(use_changed_prerequisite())
                assert events == ["native-audio"] * 4
