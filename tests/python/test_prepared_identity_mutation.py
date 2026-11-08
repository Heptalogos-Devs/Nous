"""The identity and the executed prepared request use one captured value."""

import asyncio
import json
import threading
from dataclasses import replace
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from unittest.mock import patch

import pytest
from model_support import configure_models, model_server

from nous.models.requests import PreparedGeneration
from nous.runtime.bootstrap import open_instance


def test_mutating_prepared_coverage_during_request_does_not_change_saved_identity(
    tmp_path: Path,
) -> None:
    entered, release = threading.Event(), threading.Event()
    requests: list[bytes] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            requests.append(self.rfile.read(int(self.headers["Content-Length"])))
            entered.set()
            assert release.wait(15)
            payload = json.dumps(
                {
                    "id": "mutation-test",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "local-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": "Original output."},
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
                source = await runtime.artifacts.ingest("source")
                selected = await runtime.generation.accept_inputs(source, snapshot)
                plan = await runtime.generation.prepare_input("read", selected, snapshot)
                prepared = await runtime.generation.prepare_group(
                    plan.frozen, plan.batches[0], snapshot
                )
                assert isinstance(prepared, PreparedGeneration)
                prepared = replace(prepared, coverage={"marker": "original"})
                requested = {"marker": "requested-original"}
                task = asyncio.create_task(
                    runtime.generation.produce_prepared(
                        prepared, producer_key="mutation", coverage=requested
                    )
                )
                try:
                    assert await asyncio.to_thread(entered.wait, 15)
                    prepared.coverage["marker"] = "changed"
                    requested["marker"] = "requested-changed"
                finally:
                    release.set()
                result = await task
                assert result.coverage == {"marker": "original"}
                accepted = await runtime.results.find_producer("mutation:response")
                assert accepted is not None
                assert accepted.parameters["requested_coverage"] == {"marker": "requested-original"}
                prepared.coverage["marker"] = "original"
                requested["marker"] = "requested-original"
                repeated = await runtime.generation.produce_prepared(
                    prepared, producer_key="mutation", coverage=requested
                )
                assert repeated.coverage == result.coverage
                assert len(requests) == 1

        asyncio.run(scenario())


def test_json_equivalent_requested_coverage_reuses_after_reopen(tmp_path: Path) -> None:
    requests: list[bytes] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            requests.append(self.rfile.read(int(self.headers["Content-Length"])))
            payload = json.dumps(
                {
                    "id": "coverage-json-test",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "local-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": "Output."},
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
                source = await runtime.artifacts.ingest("source")
                selected = await runtime.generation.accept_inputs(source, snapshot)
                plan = await runtime.generation.prepare_input("read", selected, snapshot)
                prepared = await runtime.generation.prepare_group(
                    plan.frozen, plan.batches[0], snapshot
                )
                assert isinstance(prepared, PreparedGeneration)
                for coverage in (
                    {"pages": (1, 2), "spans": ((1, 3),)},
                    {"pages": [1, 2], "spans": [[1, 3]]},
                ):
                    await runtime.generation.produce_prepared(
                        prepared, producer_key="json-coverage", coverage=coverage
                    )
                original_outputs = runtime.results.prepare_outputs

                async def fail_response(outputs: object) -> object:
                    if isinstance(outputs, dict) and {"response", "computation"} <= outputs.keys():
                        raise OSError("injected publication failure")
                    return await original_outputs(outputs)  # type: ignore[arg-type]

                with patch.object(runtime.results, "prepare_outputs", side_effect=fail_response):
                    with pytest.raises(OSError):
                        await runtime.generation.produce_prepared(
                            prepared,
                            producer_key="json-coverage-pending",
                            coverage={"pages": (1, 2), "spans": ((1, 3),)},
                        )
                with pytest.raises(TypeError):
                    await runtime.generation.produce_prepared(
                        prepared, producer_key="invalid-coverage", coverage={"invalid": object()}
                    )
                for value in (float("nan"), float("inf"), float("-inf")):
                    with pytest.raises(ValueError):
                        await runtime.generation.produce_prepared(
                            prepared, producer_key="nonfinite-coverage", coverage={"invalid": value}
                        )
                assert len(requests) == 2
            async with open_instance(instance, environment={}) as runtime:
                snapshot = runtime.configuration.snapshot
                plan = await runtime.generation.prepare_input("read", selected, snapshot)
                prepared = await runtime.generation.prepare_group(
                    plan.frozen, plan.batches[0], snapshot
                )
                assert isinstance(prepared, PreparedGeneration)
                for key in ("json-coverage", "json-coverage-pending"):
                    await runtime.generation.produce_prepared(
                        prepared,
                        producer_key=key,
                        coverage={"pages": (1, 2), "spans": ((1, 3),)},
                    )
                assert len(requests) == 2

        asyncio.run(scenario())
