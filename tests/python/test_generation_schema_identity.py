"""Model-visible schemas remain stable through execution and keyed recovery."""

import asyncio
import copy
import json
import threading
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from typing import cast
from unittest.mock import patch

import pytest
from model_support import configure_models, model_server
from pydantic import JsonValue, TypeAdapter

from nous.common.errors import NousError
from nous.content.contracts import SelectedInputs
from nous.models.generation import GenerationRequest
from nous.runtime.bootstrap import open_instance


def schema_for(person: str, answer: str = "A", *, reverse: bool = False) -> dict[str, JsonValue]:
    fields: dict[str, JsonValue] = {
        "answer": {
            "type": "string",
            "description": f"Return {person}'s answer",
            "examples": [f"An example for {person}"],
            "enum": [answer],
        },
        "other": {"type": "string"},
    }
    if reverse:
        fields = dict(reversed(tuple(fields.items())))
    return {
        "type": "object",
        "properties": fields,
        "required": ["answer", "other"],
        "additionalProperties": False,
    }


def answer_schema(schema: dict[str, JsonValue]) -> dict[str, JsonValue]:
    fields = cast(dict[str, JsonValue], schema["properties"])
    return cast(dict[str, JsonValue], fields["answer"])


def wire_schema(request: dict[str, JsonValue]) -> dict[str, JsonValue]:
    if "response_format" in request:
        response_format = cast(dict[str, JsonValue], request["response_format"])
        definition = cast(dict[str, JsonValue], response_format["json_schema"])
        return cast(dict[str, JsonValue], definition["schema"])
    tools = cast(list[dict[str, JsonValue]], request["tools"])
    function = cast(dict[str, JsonValue], tools[-1]["function"])
    return cast(dict[str, JsonValue], function["parameters"])


class SchemaHandler(BaseHTTPRequestHandler):
    requests: list[dict[str, JsonValue]] = []
    entered: threading.Event | None = None
    release: threading.Event | None = None

    def do_POST(self) -> None:
        request = TypeAdapter(dict[str, JsonValue]).validate_json(
            self.rfile.read(int(self.headers["Content-Length"]))
        )
        self.requests.append(request)
        if self.entered is not None:
            self.entered.set()
        if self.release is not None and not self.release.wait(15):
            return
        field = answer_schema(wire_schema(request))
        answer = cast(list[JsonValue], field["enum"])[0]
        output = json.dumps({"answer": answer, "other": field["description"]})
        is_tool = "tools" in request
        message: dict[str, JsonValue] = {"role": "assistant", "content": output}
        if is_tool:
            message = {
                "role": "assistant",
                "tool_calls": [
                    {
                        "id": "call-schema",
                        "type": "function",
                        "function": {"name": "result", "arguments": output},
                    }
                ],
            }
        body = json.dumps(
            {
                "id": "schema-identity",
                "object": "chat.completion",
                "created": 1700000000,
                "model": "local-model",
                "choices": [
                    {
                        "index": 0,
                        "message": message,
                        "finish_reason": "tool_calls" if is_tool else "stop",
                    }
                ],
            }
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        pass


def reset_handler() -> None:
    SchemaHandler.requests = []
    SchemaHandler.entered = None
    SchemaHandler.release = None


@pytest.mark.parametrize("mode", ["native", "tool"])
@pytest.mark.parametrize("api", ["produce", "run"])
def test_schema_identity_tracks_wire_request_and_reopen(
    tmp_path: Path, api: str, mode: str
) -> None:
    reset_handler()
    instance = tmp_path / "instance"
    with model_server(SchemaHandler) as server:
        configure_models(instance, server, mode)

        async def scenario() -> None:
            alice, bob = schema_for("Alice"), schema_for("Bob")
            examples = copy.deepcopy(alice)
            answer_schema(examples)["examples"] = ["A different example"]
            reordered = schema_for("Alice", reverse=True)
            selected: SelectedInputs

            async with open_instance(instance, environment={}) as runtime:
                snapshot = runtime.configuration.snapshot
                source = await runtime.artifacts.ingest("Biographical source")
                selected = await runtime.generation.accept_inputs(source, snapshot)

                async def call(schema: dict[str, JsonValue], key: str) -> dict[str, JsonValue]:
                    request = GenerationRequest("read", selected, schema=schema)
                    if api == "produce":
                        produced = await runtime.generation.produce(
                            request, computation_key=key, snapshot=snapshot
                        )
                        body = produced.outputs["output"].data.decode()
                    else:
                        result = await runtime.generation.run(
                            request, snapshot, computation_key=key
                        )
                        artifact = next(o.artifact for o in result.outputs if o.name == "output")
                        body = await runtime.artifacts.read_text(artifact)
                    return TypeAdapter(dict[str, JsonValue]).validate_json(body)

                first, *followers = await asyncio.gather(*(call(alice, "same") for _ in range(5)))
                assert all(value == first for value in followers)
                assert first["other"] == "Return Alice's answer"
                assert len(SchemaHandler.requests) == 1
                same = GenerationRequest("read", selected, schema=copy.deepcopy(alice))
                if api == "produce":
                    await runtime.generation.run(same, snapshot, computation_key="same")
                else:
                    await runtime.generation.produce(
                        same, computation_key="same", snapshot=snapshot
                    )
                assert len(SchemaHandler.requests) == 1
                for changed in (bob, examples, reordered):
                    with pytest.raises(NousError) as conflict:
                        await call(changed, "same")
                    assert conflict.value.code == "content.producer_conflict"
                    assert len(SchemaHandler.requests) == 1
                distinct = await call(bob, "different")
                assert distinct["other"] == "Return Bob's answer"
                assert answer_schema(wire_schema(SchemaHandler.requests[-1]))["description"] == (
                    "Return Bob's answer"
                )
                await call(reordered, "reordered")
                assert list(
                    cast(
                        dict[str, JsonValue], wire_schema(SchemaHandler.requests[-1])["properties"]
                    )
                ) == ["other", "answer"]
                assert len(SchemaHandler.requests) == 3

            async with open_instance(instance, environment={}) as runtime:
                snapshot = runtime.configuration.snapshot
                request = GenerationRequest("read", selected, schema=alice)
                if api == "produce":
                    recovered = await runtime.generation.produce(
                        request, computation_key="same", snapshot=snapshot
                    )
                    assert (
                        TypeAdapter(dict[str, JsonValue]).validate_json(
                            recovered.outputs["output"].data
                        )
                        == first
                    )
                    changed_request = GenerationRequest("read", selected, schema=bob)
                    with pytest.raises(NousError) as conflict:
                        await runtime.generation.produce(
                            changed_request, computation_key="same", snapshot=snapshot
                        )
                else:
                    recovered = await runtime.generation.run(
                        request, snapshot, computation_key="same"
                    )
                    artifact = next(o.artifact for o in recovered.outputs if o.name == "output")
                    assert (
                        TypeAdapter(dict[str, JsonValue]).validate_json(
                            await runtime.artifacts.read_text(artifact)
                        )
                        == first
                    )
                    with pytest.raises(NousError) as conflict:
                        await runtime.generation.run(
                            GenerationRequest("read", selected, schema=bob),
                            snapshot,
                            computation_key="same",
                        )
                assert conflict.value.code == "content.producer_conflict"
                assert len(SchemaHandler.requests) == 3

        asyncio.run(scenario())


@pytest.mark.parametrize("api", ["produce", "run"])
def test_nested_schema_mutation_during_preparation_uses_captured_request(
    tmp_path: Path, api: str
) -> None:
    reset_handler()
    instance = tmp_path / "instance"
    with model_server(SchemaHandler) as server:
        configure_models(instance, server, "native")

        async def scenario() -> None:
            async with open_instance(instance, environment={}) as runtime:
                snapshot = runtime.configuration.snapshot
                source = await runtime.artifacts.ingest("Source")
                selected = await runtime.generation.accept_inputs(source, snapshot)
                schema = schema_for("Alice", "A")
                original = copy.deepcopy(schema)
                entered, release = asyncio.Event(), asyncio.Event()
                prepare = runtime.generation.prepare_input

                async def pause(*args: object, **kwargs: object) -> object:
                    entered.set()
                    await release.wait()
                    return await prepare(*args, **kwargs)  # type: ignore[arg-type]

                async def call(value: dict[str, JsonValue]) -> object:
                    request = GenerationRequest("read", selected, schema=value)
                    if api == "produce":
                        return await runtime.generation.produce(
                            request, computation_key="mutated", snapshot=snapshot
                        )
                    return await runtime.generation.run(
                        request, snapshot, computation_key="mutated"
                    )

                with patch.object(runtime.generation, "prepare_input", new=pause):
                    task = asyncio.create_task(call(schema))
                    try:
                        await asyncio.wait_for(entered.wait(), 5)
                        cast(list[JsonValue], answer_schema(schema)["enum"])[0] = "B"
                    finally:
                        release.set()
                    first = await task
                assert answer_schema(wire_schema(SchemaHandler.requests[0]))["enum"] == ["A"]
                assert await call(original) == first
                with pytest.raises(NousError) as conflict:
                    await call(schema)
                assert conflict.value.code == "content.producer_conflict"
                assert len(SchemaHandler.requests) == 1

        asyncio.run(scenario())


@pytest.mark.parametrize("api", ["produce", "run"])
def test_queued_same_key_captures_schema_before_waiting(tmp_path: Path, api: str) -> None:
    reset_handler()
    SchemaHandler.entered, SchemaHandler.release = threading.Event(), threading.Event()
    instance = tmp_path / "instance"
    with model_server(SchemaHandler) as server:
        configure_models(instance, server, "native")

        async def scenario() -> None:
            async with open_instance(instance, environment={}) as runtime:
                snapshot = runtime.configuration.snapshot
                source = await runtime.artifacts.ingest("Source")
                selected = await runtime.generation.accept_inputs(source, snapshot)
                leader_schema, follower_schema = schema_for("Alice"), schema_for("Alice")

                async def call(schema: dict[str, JsonValue]) -> object:
                    request = GenerationRequest("read", selected, schema=schema)
                    if api == "produce":
                        return await runtime.generation.produce(
                            request, computation_key="queued", snapshot=snapshot
                        )
                    return await runtime.generation.run(request, snapshot, computation_key="queued")

                leader = asyncio.create_task(call(leader_schema))
                follower = None
                conflicting = None
                try:
                    assert SchemaHandler.entered is not None
                    assert await asyncio.to_thread(SchemaHandler.entered.wait, 10)
                    follower = asyncio.create_task(call(follower_schema))
                    for _ in range(200):
                        if runtime.execution.in_flight >= 2:
                            break
                        await asyncio.sleep(0.005)
                    assert runtime.execution.in_flight >= 2
                    cast(list[JsonValue], answer_schema(follower_schema)["enum"])[0] = "B"
                    conflicting = asyncio.create_task(call(schema_for("Bob")))
                    for _ in range(200):
                        if runtime.execution.in_flight >= 3:
                            break
                        await asyncio.sleep(0.005)
                    assert runtime.execution.in_flight >= 3
                    assert SchemaHandler.release is not None
                    SchemaHandler.release.set()
                    first, second, third = await asyncio.wait_for(
                        asyncio.gather(leader, follower, conflicting, return_exceptions=True), 10
                    )
                    assert first == second
                    assert (
                        isinstance(third, NousError) and third.code == "content.producer_conflict"
                    )
                    assert len(SchemaHandler.requests) == 1
                finally:
                    if SchemaHandler.release is not None:
                        SchemaHandler.release.set()
                    for task in (leader, follower, conflicting):
                        if task is not None and not task.done():
                            task.cancel()
                    await asyncio.gather(
                        *(task for task in (leader, follower, conflicting) if task is not None),
                        return_exceptions=True,
                    )

        asyncio.run(scenario())


@pytest.mark.parametrize("api", ["produce", "run"])
def test_pending_response_checks_full_schema_after_reopen(tmp_path: Path, api: str) -> None:
    reset_handler()
    instance = tmp_path / "instance"
    with model_server(SchemaHandler) as server:
        configure_models(instance, server, "native")

        async def scenario() -> None:
            alice, bob = schema_for("Alice"), schema_for("Bob")
            async with open_instance(instance, environment={}) as runtime:
                snapshot = runtime.configuration.snapshot
                source = await runtime.artifacts.ingest("Source")
                selected = await runtime.generation.accept_inputs(source, snapshot)
                request = GenerationRequest("read", selected, schema=alice)
                prepare_outputs = runtime.results.prepare_outputs

                async def fail_response(outputs: object) -> object:
                    if isinstance(outputs, dict) and {"response", "computation"} <= outputs.keys():
                        raise OSError("response publication interrupted")
                    return await prepare_outputs(outputs)  # type: ignore[arg-type]

                with patch.object(runtime.results, "prepare_outputs", side_effect=fail_response):
                    with pytest.raises(OSError, match="response publication interrupted"):
                        if api == "produce":
                            await runtime.generation.produce(
                                request, computation_key="pending", snapshot=snapshot
                            )
                        else:
                            await runtime.generation.run(
                                request, snapshot, computation_key="pending"
                            )
                assert len(SchemaHandler.requests) == 1

            async with open_instance(instance, environment={}) as runtime:
                snapshot = runtime.configuration.snapshot
                with pytest.raises(NousError) as conflict:
                    changed = GenerationRequest("read", selected, schema=bob)
                    if api == "produce":
                        await runtime.generation.produce(
                            changed, computation_key="pending", snapshot=snapshot
                        )
                    else:
                        await runtime.generation.run(changed, snapshot, computation_key="pending")
                assert conflict.value.code == "content.producer_conflict"
                request = GenerationRequest("read", selected, schema=alice)
                if api == "produce":
                    result = await runtime.generation.produce(
                        request, computation_key="pending", snapshot=snapshot
                    )
                    output = result.outputs["output"].data
                else:
                    result = await runtime.generation.run(
                        request, snapshot, computation_key="pending"
                    )
                    artifact = next(o.artifact for o in result.outputs if o.name == "output")
                    output = (await runtime.artifacts.read_text(artifact)).encode()
                assert TypeAdapter(dict[str, JsonValue]).validate_json(output)["other"] == (
                    "Return Alice's answer"
                )
                assert len(SchemaHandler.requests) == 1

        asyncio.run(scenario())
