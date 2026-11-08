"""Recover the caller's explicit prepared-input interpretation after interruption."""

from __future__ import annotations

import asyncio
import json
from dataclasses import replace
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from unittest.mock import patch

import pytest
from model_support import configure_models, model_server
from pydantic_ai.messages import BinaryContent

from nous.common.errors import NousError
from nous.models.generation import PendingResponse
from nous.models.requests import PreparedGeneration
from nous.runtime.bootstrap import open_instance


def test_prepared_response_retains_requested_inputs_and_coverage(tmp_path: Path) -> None:
    requests: list[bytes] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            requests.append(self.rfile.read(int(self.headers["Content-Length"])))
            payload = json.dumps(
                {
                    "id": "prepared-recovery",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "local-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": "Prepared output."},
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

    instance = tmp_path / "prepared-recovery"
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def scenario() -> None:
            async with open_instance(instance, environment={}) as runtime:
                snapshot = runtime.configuration.snapshot
                source = await runtime.artifacts.ingest("The source of the prepared response.")
                selected = await runtime.generation.accept_inputs(source, snapshot)
                plan = await runtime.generation.prepare_input("read", selected, snapshot)
                prepared = await runtime.generation.prepare_group(
                    plan.frozen, plan.batches[0], snapshot
                )
                assert isinstance(prepared, PreparedGeneration)
                group = prepared.compiled.reading.sources[0].group
                assert group is not None
                override = group.model_copy(update={"key": "S9", "slot_key": "S9"})
                prepared = replace(prepared, coverage={})
                original = runtime.results.prepare_outputs

                async def fail_response(outputs: object) -> object:
                    if isinstance(outputs, dict) and {"response", "computation"} <= outputs.keys():
                        raise OSError("response publication failed")
                    return await original(outputs)  # type: ignore[arg-type]

                with patch.object(runtime.results, "prepare_outputs", side_effect=fail_response):
                    with pytest.raises(OSError) as failed:
                        await runtime.generation.produce_prepared(
                            prepared,
                            inputs=(override,),
                            coverage={"marker": "retained"},
                            producer_key="prepared:override",
                        )
                pending = failed.value.__dict__.get("nous_recovery")
                assert isinstance(pending, PendingResponse)
                assert len(requests) == 1
            async with open_instance(instance, environment={}) as runtime:
                key = "prepared:override:response"
                assert await runtime.generation.responses.pending_for(key) is not None
                reading = pending.reading
                source = replace(
                    reading.sources[0],
                    media=(BinaryContent(data=b"image", media_type="image/png"),),
                )
                reading_with_buffer = replace(reading, sources=(source,))
                in_memory = replace(
                    pending,
                    reading=reading_with_buffer,
                    generated=replace(pending.generated, reading=reading_with_buffer),
                )
                restored = await runtime.generation.resume_response(
                    in_memory, runtime.configuration.snapshot
                )
                assert restored.actual_inputs == (override,)
                assert restored.coverage == {"marker": "retained"}
                response = await runtime.results.find_producer("prepared:override:response")
                assert response is not None
                repeated = await runtime.generation.recover_response(
                    response.ref, runtime.configuration.snapshot
                )
                assert repeated.actual_inputs == restored.actual_inputs
                assert repeated.coverage == restored.coverage
                results = await asyncio.gather(
                    *(
                        runtime.generation.recover_result(key, runtime.configuration.snapshot)
                        for _ in range(8)
                    )
                )
                assert len({result.ref.id for result in results}) == 1
                for changed in (
                    replace(pending, requested_inputs=(group,)),
                    replace(pending, requested_coverage={"marker": "different"}),
                ):
                    with pytest.raises(NousError) as conflict:
                        await runtime.generation.resume_response(
                            changed, runtime.configuration.snapshot
                        )
                    assert conflict.value.code == "content.producer_conflict"
                assert len(requests) == 1

        asyncio.run(scenario())
