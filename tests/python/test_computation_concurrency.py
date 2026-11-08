"""Concurrent computations reuse durable results without merging distinct work."""

import asyncio
import json
import shutil
import subprocess
import sys
import wave
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from unittest.mock import patch

import pytest
from model_support import configure_models, model_server

from nous.application.configuration import open_configuration_operations
from nous.common.errors import NousError
from nous.common.operations import exception_context
from nous.content.contracts import OutputContent, SourceSelection
from nous.media.audiovisual import AudioVisual
from nous.media.derivatives import MediaDerivatives
from nous.models.generation import GenerationRequest
from nous.runtime.bootstrap import open_instance


def test_generation_same_key_reuses_and_checks_identity(tmp_path: Path) -> None:
    requests: list[dict[str, object]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            requests.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            payload = json.dumps(
                {
                    "id": "test",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "local-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": "successful output"},
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

        async def scenario() -> None:
            async with open_instance(instance, environment={}) as runtime:
                snapshot = runtime.configuration.snapshot
                source = await runtime.artifacts.ingest("one source")
                selected = await runtime.generation.accept_inputs(source, snapshot)
                request = GenerationRequest("read", selected)
                values = await asyncio.gather(
                    *(
                        runtime.generation.produce(
                            request, computation_key="same", snapshot=snapshot
                        )
                        for _ in range(4)
                    )
                )
                assert len(requests) == 1
                assert all(value.outputs["output"].data == b"successful output" for value in values)
                await runtime.generation.produce(request, computation_key="same", snapshot=snapshot)
                assert len(requests) == 1
                other = GenerationRequest("read", selected, question="a different request")
                with pytest.raises(NousError) as conflict:
                    await runtime.generation.produce(
                        other, computation_key="same", snapshot=snapshot
                    )
                assert conflict.value.code == "content.producer_conflict"
                assert len(requests) == 1
                prepare_outputs = runtime.results.prepare_outputs

                async def fail_response(outputs: object) -> object:
                    if isinstance(outputs, dict) and {"response", "computation"} <= outputs.keys():
                        raise OSError("injected response publication failure")
                    return await prepare_outputs(outputs)  # type: ignore[arg-type]

                with patch.object(runtime.results, "prepare_outputs", side_effect=fail_response):
                    with pytest.raises(OSError):
                        await runtime.generation.produce(
                            request, computation_key="pending", snapshot=snapshot
                        )
                assert len(requests) == 2
                restored = await runtime.generation.produce(
                    request, computation_key="pending", snapshot=snapshot
                )
                assert restored.outputs["output"].data == b"successful output"
                assert len(requests) == 2

        asyncio.run(scenario())


def test_derivative_failure_and_cancellation_release_same_key(tmp_path: Path) -> None:
    from nous.runtime.bootstrap import initialize_instance

    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            original = await runtime.artifacts.ingest("original")
            derivatives = MediaDerivatives(runtime.artifacts, runtime.results)
            output = {
                "value": OutputContent(
                    data=b"prepared", media_type="text/plain", display_name="prepared.txt"
                )
            }
            adopt = runtime.results.adopt
            attempts = 0

            async def fail_once(*args: object, **kwargs: object) -> object:
                nonlocal attempts
                attempts += 1
                if attempts == 1:
                    raise OSError("temporary failure")
                return await adopt(*args, **kwargs)  # type: ignore[arg-type]

            with patch.object(runtime.results, "adopt", side_effect=fail_once):
                with pytest.raises(OSError):
                    await derivatives.derive(
                        original, SourceSelection(), "test.derivative", output, {}
                    )
                result = await derivatives.derive(
                    original, SourceSelection(), "test.derivative", output, {}
                )
                same = await derivatives.derive(
                    original, SourceSelection(), "test.derivative", output, {}
                )
            assert result["value"].ref == same["value"].ref and attempts == 2
            started, release = asyncio.Event(), asyncio.Event()

            async def delayed(*args: object, **kwargs: object) -> object:
                started.set()
                await release.wait()
                return await adopt(*args, **kwargs)  # type: ignore[arg-type]

            with patch.object(runtime.results, "adopt", side_effect=delayed):
                first = asyncio.create_task(
                    derivatives.derive(original, SourceSelection(), "test.cancel", output, {})
                )
                await started.wait()
                second = asyncio.create_task(
                    derivatives.derive(original, SourceSelection(), "test.cancel", output, {})
                )
                first.cancel()
                with pytest.raises(asyncio.CancelledError):
                    await first
                release.set()
                recovered = await second
            assert await runtime.artifacts.read_bytes(recovered["value"].ref) == b"prepared"
            converting = asyncio.Event()

            async def interrupted_conversion() -> dict[str, OutputContent]:
                converting.set()
                await asyncio.Event().wait()
                return output

            async def retry_conversion() -> dict[str, OutputContent]:
                return output

            first = asyncio.create_task(
                derivatives.get_or_create(
                    original,
                    SourceSelection(),
                    "test.cancel_conversion",
                    {},
                    interrupted_conversion,
                )
            )
            await converting.wait()
            second = asyncio.create_task(
                derivatives.get_or_create(
                    original, SourceSelection(), "test.cancel_conversion", {}, retry_conversion
                )
            )
            first.cancel()
            with pytest.raises(asyncio.CancelledError):
                await first
            assert await runtime.artifacts.read_bytes((await second)["value"].ref) == b"prepared"

    asyncio.run(scenario())


@pytest.mark.skipif(
    not shutil.which("ffmpeg") or not shutil.which("ffprobe"),
    reason="FFmpeg and ffprobe are required",
)
def test_parallel_audio_and_cli_publication_recovery(tmp_path: Path) -> None:
    requests: list[str] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            self.rfile.read(int(self.headers["Content-Length"]))
            requests.append(self.path)
            value = (
                {"text": "durable transcript"}
                if self.path.endswith("/audio/transcriptions")
                else {
                    "id": "test",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "local-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": "audio description"},
                            "finish_reason": "stop",
                        }
                    ],
                }
            )
            payload = json.dumps(value).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, format: str, *args: object) -> None:
            pass

    instance = tmp_path / "instance"
    audio = tmp_path / "speech.wav"
    with wave.open(str(audio), "wb") as stream:
        stream.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
        stream.writeframes(b"\0\0" * 16000)
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def configure() -> None:
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
                        "tasks.describe": {"model": "reader", "prompt": "describe-media"},
                        "media.audio.transcription_task": "asr",
                    }
                )

        asyncio.run(configure())

        async def concurrent() -> str:
            async with open_instance(instance, environment={}) as runtime:
                source = await runtime.artifacts.ingest(audio)
                original_audio = AudioVisual.audio
                audio_calls = 0

                async def counted_audio(
                    owner: AudioVisual,
                    path: Path,
                    start_ms: int,
                    end_ms: int,
                    media_type: str = "audio/wav",
                ) -> bytes:
                    nonlocal audio_calls
                    audio_calls += 1
                    return await original_audio(owner, path, start_ms, end_ms, media_type)

                with patch.object(AudioVisual, "audio", new=counted_audio):
                    values = await asyncio.gather(
                        *(
                            runtime.media.analyze(
                                source.ref,
                                "description_only",
                                runtime.configuration.snapshot,
                                description_task="describe",
                            )
                            for _ in range(8)
                        )
                    )
                assert all(value.batches for value in values)
                assert audio_calls == 1
                retry_source = await runtime.artifacts.ingest(audio)
                return retry_source.ref.identifier

        retry_ref = asyncio.run(concurrent())
        assert sum(path.endswith("/audio/transcriptions") for path in requests) == 1
        assert sum(path.endswith("/chat/completions") for path in requests) == 8
        program = """from nous.content.results import ResultStore
original=ResultStore.adopt
async def fail(self,*args,**kwargs):
    if kwargs.get("operation")=="asr": raise OSError(28, "injected no space")
    return await original(self,*args,**kwargs)
ResultStore.adopt=fail
from nous.transports.cli import main
main()
"""
        args = [
            "--instance",
            str(instance),
            "--json",
            "tools",
            "media",
            "analyze",
            retry_ref,
            "--strategy",
            "description_only",
            "--description-task",
            "describe",
        ]
        failed = subprocess.run(
            [sys.executable, "-c", program, *args],
            capture_output=True,
            text=True,
            timeout=45,
            check=False,
        )
        assert failed.returncode != 0
        failure = json.loads(failed.stdout)
        details = failure.get("error", failure).get("details", {})
        assert details["artifact"] == retry_ref
        assert details["model_succeeded"] is True
        assert details["transcription_recovery_durable"] is True
        assert details["producer_key"].startswith("transcription-response:")
        assert "original accepted Artifact" in details["recovery_instruction"]
        assert sum(path.endswith("/audio/transcriptions") for path in requests) == 2
        retried = subprocess.run(
            [sys.executable, "-c", "from nous.transports.cli import main; main()", *args],
            capture_output=True,
            text=True,
            timeout=45,
            check=False,
        )
        assert retried.returncode == 0, (retried.stdout, retried.stderr)
        assert sum(path.endswith("/audio/transcriptions") for path in requests) == 2

        async def fail_before_journal() -> None:
            async with open_instance(instance, environment={}) as runtime:
                source = await runtime.artifacts.ingest(audio)
                journal = runtime.generation.transcription.journal
                with patch.object(journal, "save", side_effect=OSError("journal unavailable")):
                    with pytest.raises(OSError) as failure:
                        await runtime.media.analyze(
                            source.ref,
                            "description_only",
                            runtime.configuration.snapshot,
                            description_task="describe",
                        )
                details = exception_context(failure.value)
                assert details["model_succeeded"] is True
                assert details["transcription_recovery_durable"] is False
                assert details["artifact"] == source.ref.identifier
                assert "may repeat the model request" in str(details["recovery_instruction"])
                assert (
                    await runtime.media.analyze(
                        source.ref,
                        "description_only",
                        runtime.configuration.snapshot,
                        description_task="describe",
                    )
                ).batches

        asyncio.run(fail_before_journal())
        assert sum(path.endswith("/audio/transcriptions") for path in requests) == 3
