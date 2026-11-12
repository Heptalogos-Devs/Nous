"""Fallback results remain readable while new requests use current configuration."""

import asyncio
import json
from http.server import BaseHTTPRequestHandler
from pathlib import Path

from nous.content.contracts import ArtifactInput, ReadResult
from nous.content.operations import READ_RESULT
from nous.models.generation.contracts import RunModel
from nous.models.generation.operations import MODEL_RUN
from nous.runtime.bootstrap import open_instance
from tests.integration.shared.models import configure_models, model_server


def test_fallback_result_survives_reopen_and_new_requests_use_current_settings(
    tmp_path: Path,
) -> None:
    requests: list[dict[str, object]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            requests.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            if len(requests) == 1:
                status = 503
                payload = json.dumps({"error": {"message": "Primary unavailable"}}).encode()
            else:
                status = 200
                payload = json.dumps(
                    {
                        "id": "fallback-test",
                        "object": "chat.completion",
                        "created": 1700000000,
                        "model": "local-model",
                        "choices": [
                            {
                                "index": 0,
                                "message": {"role": "assistant", "content": "Fallback output."},
                                "finish_reason": "stop",
                            }
                        ],
                    }
                ).encode()
            self.send_response(status)
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
                await runtime.configuration.update(
                    {
                        "presets.primary": {"temperature": 0.1},
                        "presets.backup": {"temperature": 0.7},
                        "tasks.read.preset": "primary",
                        "tasks.read.fallback": [{"model": "reader", "preset": "backup"}],
                    }
                )
                source = await runtime.artifacts.ingest("fallback source")
                request = RunModel(task="read", sources=(ArtifactInput(artifact=source.ref),))
                result = await runtime.app.call(MODEL_RUN, request)
                saved_ref = result.results[0]
                view = await runtime.app.call(READ_RESULT, ReadResult(ref=saved_ref))
                assert view.outputs[0].view.body_text == "Fallback output."
                assert len(requests) == 2
                assert [request["temperature"] for request in requests] == [0.1, 0.7]
            async with open_instance(instance, environment={}) as runtime:
                await runtime.configuration.update({"presets.primary.temperature": 0.2})
                restored = await runtime.app.call(READ_RESULT, ReadResult(ref=saved_ref))
                assert restored.outputs[0].view.body_text == "Fallback output."
                assert len(requests) == 2
                await runtime.app.call(MODEL_RUN, request)
                assert len(requests) == 3
                assert requests[-1]["temperature"] == 0.2

        asyncio.run(scenario())
