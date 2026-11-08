"""A successful fallback response can be restored after reopening."""

import asyncio
import json
from http.server import BaseHTTPRequestHandler
from pathlib import Path

from model_support import configure_models, model_server

from nous.models.generation import GenerationRequest
from nous.runtime.bootstrap import open_instance


def test_fallback_response_is_reused_after_reopen(tmp_path: Path) -> None:
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
                snapshot = runtime.configuration.snapshot
                source = await runtime.artifacts.ingest("fallback source")
                selected = await runtime.generation.accept_inputs(source, snapshot)
                request = GenerationRequest("read", selected)
                result = await runtime.generation.produce(
                    request, computation_key="fallback", snapshot=snapshot
                )
                assert result.outputs["output"].data == b"Fallback output."
                assert len(requests) == 2
                assert [request["temperature"] for request in requests] == [0.1, 0.7]
            async with open_instance(instance, environment={}) as runtime:
                await runtime.configuration.update({"presets.primary.temperature": 0.2})
                snapshot = runtime.configuration.snapshot
                restored = await runtime.generation.produce(
                    request, computation_key="fallback", snapshot=snapshot
                )
                assert restored.outputs["output"].data == b"Fallback output."
                assert len(requests) == 2
                await runtime.generation.produce(
                    request, computation_key="new-computation", snapshot=snapshot
                )
                assert len(requests) == 3
                assert requests[-1]["temperature"] == 0.2

        asyncio.run(scenario())
