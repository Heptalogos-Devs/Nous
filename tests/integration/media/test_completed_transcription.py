"""Completed transcriptions remain useful after a later operation fails."""

import asyncio
import json
import shutil
import wave
from http.server import BaseHTTPRequestHandler
from pathlib import Path

import pytest

from nous.application.configuration import open_configuration_operations
from nous.application.inputs import FileInput
from nous.common.errors import NousError
from nous.content.contracts import (
    AcceptContent,
    ArtifactInput,
    ReadResult,
    ResultRef,
    SourceSelection,
)
from nous.content.operations import ACCEPT_CONTENT, READ_RESULT
from nous.media.analysis import ANALYZE_MEDIA, AnalyzeMedia
from nous.runtime.bootstrap import open_instance
from tests.integration.shared.models import configure_models, model_server


def test_saved_transcript_is_reused_after_description_failure(tmp_path: Path) -> None:
    if not shutil.which("ffmpeg") or not shutil.which("ffprobe"):
        pytest.skip("Audio preparation requires FFmpeg and ffprobe")
    calls: list[str] = []
    description_available = False

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            nonlocal description_available
            self.rfile.read(int(self.headers["Content-Length"]))
            if self.path.endswith("/audio/transcriptions"):
                calls.append("transcription")
                status = 200
                result: dict[str, object] = {"text": "A transcript of the selected interval."}
            else:
                calls.append("description")
                status = 200 if description_available else 503
                result = (
                    {
                        "id": "description",
                        "object": "chat.completion",
                        "created": 1700000000,
                        "model": "local-model",
                        "choices": [
                            {
                                "index": 0,
                                "message": {"role": "assistant", "content": "Audio description."},
                                "finish_reason": "stop",
                            }
                        ],
                    }
                    if description_available
                    else {"error": {"message": "temporarily unavailable"}}
                )
            payload = json.dumps(result).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, format: str, *args: object) -> None:
            pass

    audio = tmp_path / "audio.wav"
    with wave.open(str(audio), "wb") as stream:
        stream.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
        stream.writeframes(b"\0\0" * 16000 * 3)
    instance = tmp_path / "instance"
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def scenario() -> ResultRef:
            nonlocal description_available
            async with open_configuration_operations(instance, environment={}) as config:
                await config.update(
                    {
                        "models.asr": {
                            "provider": "local",
                            "model_name": "asr",
                            "capabilities": ["audio"],
                            "input_formats": ["audio/wav"],
                        },
                        "tasks.asr": {"model": "asr", "operation": "transcription"},
                        "media.audio.transcription_task": "asr",
                        "model_execution.max_attempts": 1,
                    }
                )
            interval = SourceSelection(start_ms=1000, end_ms=2000)
            async with open_instance(instance, environment={}) as runtime:
                first = await runtime.app.call(
                    ACCEPT_CONTENT, AcceptContent(key="audio"), inputs={"audio": FileInput(audio)}
                )
                second = await runtime.app.call(
                    ACCEPT_CONTENT, AcceptContent(key="audio"), inputs={"audio": FileInput(audio)}
                )
                assert first.ref != second.ref
                with pytest.raises(NousError):
                    await runtime.app.call(
                        ANALYZE_MEDIA,
                        AnalyzeMedia(
                            source=ArtifactInput(artifact=first.ref, selection=interval),
                            strategy="description_only",
                            description_task="read",
                        ),
                    )
                assert calls == ["transcription", "description"]
            description_available = True
            async with open_instance(instance, environment={}) as runtime:
                result = await runtime.app.call(
                    ANALYZE_MEDIA,
                    AnalyzeMedia(
                        source=ArtifactInput(artifact=first.ref, selection=interval),
                        strategy="description_only",
                        description_task="read",
                    ),
                )
                view = await runtime.app.call(READ_RESULT, ReadResult(ref=result.descriptions[0]))
                transcript = next(
                    part
                    for group in view.inputs
                    for part in group.parts
                    if part.representation == "transcript"
                )
                origin = transcript.origins[0]
                assert origin.precision == "atomic"
                assert origin.origin.artifact == first.ref
                assert origin.origin.selection == interval
                assert origin.transform_ref is not None
                assert calls == ["transcription", "description", "description"]
                await runtime.app.call(
                    ANALYZE_MEDIA,
                    AnalyzeMedia(
                        source=ArtifactInput(artifact=second.ref, selection=interval),
                        strategy="description_only",
                        description_task="read",
                    ),
                )
                assert calls.count("transcription") == 2
                return origin.transform_ref

        transcript_ref = asyncio.run(scenario())

    async def read_after_provider_stops() -> None:
        async with open_instance(instance, environment={}) as runtime:
            view = await runtime.app.call(READ_RESULT, ReadResult(ref=transcript_ref))
            assert view.outputs[0].view.body_text == "A transcript of the selected interval."
            assert view.mappings[0].relation == "atomic"
            assert view.mappings[0].origins[0].selection == SourceSelection(
                start_ms=1000, end_ms=2000
            )

    asyncio.run(read_after_provider_stops())
