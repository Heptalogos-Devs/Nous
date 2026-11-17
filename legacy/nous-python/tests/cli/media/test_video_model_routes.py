"""Video route selection, physical representations and source coverage."""

import asyncio
import base64
import json
import shutil
import subprocess
from http.server import BaseHTTPRequestHandler
from pathlib import Path

import pytest
from pydantic import JsonValue, TypeAdapter

from nous.application.configuration import open_configuration_operations
from tests.integration.shared.models import configure_models, model_cli, model_server


@pytest.mark.parametrize("without_audio", [False, True])
def test_native_video_interval_uses_video_content_and_preserves_audio_policy(
    tmp_path: Path, without_audio: bool
) -> None:
    if not shutil.which("ffmpeg") or not shutil.which("ffprobe"):
        pytest.skip("Video interval preparation needs FFmpeg and ffprobe")
    video = tmp_path / "source.mp4"
    subprocess.run(
        [
            "ffmpeg",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=32x32:r=20",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=16000",
            "-t",
            "2",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            str(video),
        ],
        check=True,
    )
    opaque_video = tmp_path / "source-without-extension"
    video.rename(opaque_video)
    video = opaque_video
    submissions: list[dict[str, JsonValue]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            request = TypeAdapter(dict[str, JsonValue]).validate_json(
                self.rfile.read(int(self.headers["Content-Length"]))
            )
            submissions.append(request)
            payload = json.dumps(
                {
                    "id": "video-response",
                    "created": 1700000000,
                    "object": "chat.completion",
                    "model": request["model"],
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": "Video received."},
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

    instance = tmp_path / "instance"
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def configure() -> None:
            async with open_configuration_operations(instance, environment={}) as configuration:
                await configuration.update(
                    {
                        "models.reader.capabilities": ["text", "image"],
                        "models.reader.input_formats": ["text/plain", "image/png"],
                        "localization.language": "en" if without_audio else "zh-CN",
                        "models.video": {
                            "provider": "local",
                            "model_name": "local-video",
                            "capabilities": ["text", "video"],
                            "input_formats": ["text/plain", "video/mp4"],
                        },
                        "tasks.read.fallback": [{"model": "video"}],
                    }
                )

        asyncio.run(configure())
        response = model_cli(
            instance,
            "tools",
            "media",
            "analyze",
            str(video),
            "--strategy",
            "description_only",
            "--description-task",
            "read",
            "--from",
            "0.25",
            "--to",
            "0.75",
            *(["--without-audio"] if without_audio else []),
        )
        assert response.returncode == 0, (response.stdout, response.stderr)
    result = json.loads(response.stdout)["results"][0]
    assert len(submissions) == 1
    assert submissions[0]["model"] == "local-video"
    messages = TypeAdapter(list[dict[str, JsonValue]]).validate_python(submissions[0]["messages"])
    parts = [
        part
        for message in messages
        if isinstance(message["content"], list)
        for part in TypeAdapter(list[dict[str, JsonValue]]).validate_python(message["content"])
    ]
    binary = [part for part in parts if part["type"] != "text"]
    source_text = "\n".join(str(part["text"]) for part in parts if part["type"] == "text")
    if without_audio:
        assert "Source Video duration: approximately 2 seconds." in source_text
        assert "Provided Video: source interval 0.25–0.75 seconds." in source_text
        assert "The source audio was excluded from the video." in source_text
    else:
        assert "源视频时长约 2 秒。" in source_text
        assert "本次提供源文件第 0.25–0.75 秒的视频。" in source_text
        assert "移除源音轨" not in source_text
    assert [part["type"] for part in binary] == ["video_url"]
    video_url = TypeAdapter(dict[str, JsonValue]).validate_python(binary[0]["video_url"])
    payload = TypeAdapter(str).validate_python(video_url["url"])
    assert payload.startswith("data:video/mp4;base64,")
    sent = tmp_path / "sent.mp4"
    sent.write_bytes(base64.b64decode(payload.split(",", 1)[1]))
    info = json.loads(
        subprocess.check_output(
            ["ffprobe", "-v", "error", "-show_streams", "-show_format", "-of", "json", str(sent)]
        )
    )
    assert abs(float(info["format"]["duration"]) - 0.5) < 0.1
    assert any(stream["codec_type"] == "audio" for stream in info["streams"]) is not without_audio
    origin = result["inputs"][0]["parts"][0]["origins"][0]["origin"]
    assert origin["selection"]["start_ms"] == 250 and origin["selection"]["end_ms"] == 750
    assert result["inputs"][0]["context"]["audio"] == ("excluded" if without_audio else "included")
    assert "data:video" not in response.stdout


def test_video_fallback_preserves_scope_across_physical_batches(tmp_path: Path) -> None:
    if not shutil.which("ffmpeg") or not shutil.which("ffprobe"):
        pytest.skip("Video preparation needs FFmpeg and ffprobe")
    submissions: list[dict[str, object]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            submissions.append(request)
            failure = request["model"] == "native-video"
            data = json.dumps(
                {"error": {"message": "native unavailable"}}
                if failure
                else {
                    "id": "fallback-frames",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": request["model"],
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": "Frames received."},
                            "finish_reason": "stop",
                        }
                    ],
                }
            ).encode()
            self.send_response(503 if failure else 200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, format: str, *args: object) -> None:
            pass

    video = tmp_path / "fallback.mp4"
    subprocess.run(
        [
            "ffmpeg",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=32x32:r=20",
            "-t",
            "2",
            "-c:v",
            "libx264",
            str(video),
        ],
        check=True,
    )
    instance = tmp_path / "video-fallback"
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def configure() -> None:
            async with open_configuration_operations(instance, environment={}) as configuration:
                await configuration.update(
                    {
                        "models.native-video": {
                            "provider": "local",
                            "model_name": "native-video",
                            "capabilities": ["text", "video"],
                            "input_formats": ["text/plain", "video/mp4"],
                        },
                        "models.frames": {
                            "provider": "local",
                            "model_name": "frame-model",
                            "capabilities": ["text", "image"],
                            "input_formats": ["text/plain", "image/png"],
                        },
                        "tasks.read": {
                            "model": "native-video",
                            "prompt": "summarize",
                            "fallback": [{"model": "frames"}],
                        },
                        "media.video.sample_interval_ms": 500,
                        "media.video.max_frames_per_request": 1,
                    }
                )

        asyncio.run(configure())
        response = model_cli(
            instance,
            "tools",
            "media",
            "analyze",
            str(video),
            "--strategy",
            "description_only",
            "--description-task",
            "read",
            "--from",
            "0.25",
            "--to",
            "1.75",
            "--without-audio",
            "--per-batch",
        )
    assert response.returncode == 0, (response.stdout, response.stderr)
    outcome = json.loads(response.stdout)
    result = outcome["results"][0]
    assert [request["model"] for request in submissions] == ["native-video", "frame-model"]
    coverage = result["coverage"]
    assert coverage["mode"] == "independent"
    assert coverage["inputs"][0]["selection"]["start_ms"] == 250
    assert coverage["inputs"][0]["selection"]["end_ms"] == 1750
    assert [(item["start_ms"], item["end_ms"]) for item in coverage["actual"]] == [
        (250, 750),
        (750, 1250),
        (1250, 1750),
    ]
    assert all(item["mode"] == "sampled" for item in coverage["actual"])
    origins = [
        origin["origin"]
        for group in result["inputs"]
        for part in group["parts"]
        for origin in part["origins"]
    ]
    assert len(origins) == 3
    assert all(
        item["artifact"]["identifier"] == outcome["original"]["identifier"] for item in origins
    )
    assert all(
        250 <= item["selection"]["start_ms"] < item["selection"]["end_ms"] <= 1750
        for item in origins
    )
    reread = model_cli(
        instance, "tools", "result", "read", outcome["descriptions"][0]["identifier"]
    )
    source = model_cli(instance, "tools", "content", "read", outcome["original"]["identifier"])
    assert reread.returncode == source.returncode == 0
    result_view, source_view = json.loads(reread.stdout), json.loads(source.stdout)
    assert (
        result_view["input_views"]
        and result_view["outputs"][0]["view"]["body_text"] == "Frames received."
    )
    description = next(
        item for item in source_view["available_representations"] if item["kind"] == "description"
    )
    assert any(
        item["artifact"]["identifier"] == outcome["original"]["identifier"]
        and item["selection"]["start_ms"] == 250
        and item["selection"]["end_ms"] == 1750
        for item in description["source_selections"]
    )
