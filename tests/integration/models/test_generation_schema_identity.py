"""Model-visible schemas remain stable through execution and keyed recovery."""

import asyncio
import json
import threading
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from typing import cast
from unittest.mock import patch

import pytest
from pydantic import JsonValue, TypeAdapter

from nous.runtime.bootstrap import open_instance
from tests.integration.shared.models import configure_models, model_server


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
        if "response_format" not in request and "tools" not in request:
            output = json.dumps({"answer": "A", "other": "prompted result"})
        else:
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
def test_schema_visible_to_model_and_saved_result_survives_reopen(
    tmp_path: Path, mode: str
) -> None:
    from nous.content.contracts import InlineText, ReadResult, ResultRef
    from nous.content.operations import READ_RESULT
    from nous.models.generation.contracts import RunModel
    from nous.models.generation.operations import MODEL_RUN

    reset_handler()
    instance = tmp_path / "instance"
    with model_server(SchemaHandler) as server:
        configure_models(instance, server, mode)

        async def scenario() -> None:
            alice, bob = schema_for("Alice"), schema_for("Bob")
            reordered = schema_for("Alice", reverse=True)
            async with open_instance(instance, environment={}) as runtime:
                refs: list[ResultRef] = []
                for schema in (alice, bob, reordered):
                    outcome = await runtime.app.call(
                        MODEL_RUN,
                        RunModel(
                            task="read",
                            sources=(InlineText(text="Biographical source"),),
                            output_schema=schema,
                        ),
                    )
                    refs.append(outcome.results[0])
                assert len(SchemaHandler.requests) == 3
                assert (
                    answer_schema(wire_schema(SchemaHandler.requests[0]))["description"]
                    == "Return Alice's answer"
                )
                assert (
                    answer_schema(wire_schema(SchemaHandler.requests[1]))["description"]
                    == "Return Bob's answer"
                )
                assert list(
                    cast(dict[str, JsonValue], wire_schema(SchemaHandler.requests[2])["properties"])
                ) == ["other", "answer"]
            async with open_instance(instance, environment={}) as runtime:
                saved = await runtime.app.call(READ_RESULT, ReadResult(ref=refs[0], details=True))
                body = saved.outputs[0].view.body_text
                assert body is not None
                assert (
                    TypeAdapter(dict[str, JsonValue]).validate_json(body)["other"]
                    == "Return Alice's answer"
                )
                assert len(SchemaHandler.requests) == 3

        asyncio.run(scenario())


def test_schema_mutation_during_preparation_does_not_change_accepted_request(
    tmp_path: Path,
) -> None:
    from nous.content.contracts import InlineText
    from nous.models.generation.contracts import RunModel
    from nous.models.generation.operations import MODEL_RUN

    reset_handler()
    instance = tmp_path / "instance"
    with model_server(SchemaHandler) as server:
        configure_models(instance, server, "native")

        async def scenario() -> None:
            async with open_instance(instance, environment={}) as runtime:
                schema = schema_for("Alice", "A")
                request = RunModel(
                    task="read", sources=(InlineText(text="Source"),), output_schema=schema
                )
                entered, release = asyncio.Event(), asyncio.Event()
                prepare = runtime.generation.preparation.prepare_input

                async def pause(*args: object, **kwargs: object) -> object:
                    entered.set()
                    await release.wait()
                    return await prepare(*args, **kwargs)  # type: ignore[arg-type]

                with patch.object(runtime.generation.preparation, "prepare_input", new=pause):
                    task = asyncio.create_task(runtime.app.call(MODEL_RUN, request))
                    try:
                        await asyncio.wait_for(entered.wait(), 5)
                        request_schema = cast(dict[str, JsonValue], request.output_schema)
                        cast(list[JsonValue], answer_schema(request_schema)["enum"])[0] = "B"
                    finally:
                        release.set()
                    await task
                assert answer_schema(wire_schema(SchemaHandler.requests[0]))["enum"] == ["A"]
                await runtime.app.call(MODEL_RUN, request)
                assert answer_schema(wire_schema(SchemaHandler.requests[1]))["enum"] == ["B"]

        asyncio.run(scenario())


def test_prompted_schema_uses_text_route_and_validates_output(tmp_path: Path) -> None:
    from nous.application.configuration import open_configuration_operations
    from nous.common.errors import NousError
    from nous.content.contracts import InlineText
    from nous.models.generation.contracts import RunModel
    from nous.models.generation.operations import MODEL_RUN

    reset_handler()
    instance = tmp_path / "instance"
    with model_server(SchemaHandler) as server:
        configure_models(instance, server, "prompted")

        async def scenario() -> None:
            async with open_configuration_operations(instance, environment={}) as configuration:
                await configuration.update(
                    {
                        "models.reader.capabilities": ["text"],
                        "models.reader.output_modes": ["text"],
                        "model_execution.structured_retries": 0,
                    }
                )
            async with open_instance(instance, environment={}) as runtime:
                result = await runtime.app.call(
                    MODEL_RUN,
                    RunModel(
                        task="read",
                        sources=(InlineText(text="Source"),),
                        output_schema=schema_for("Alice", "A"),
                    ),
                )
                saved = await runtime.results.get(result.results[0])
                body = await runtime.artifacts.read_text(saved.outputs[0].artifact)
                assert json.loads(body)["answer"] == "A"
                request = SchemaHandler.requests[0]
                assert "tools" not in request and "response_format" not in request
                assert "Return Alice's answer" in json.dumps(request["messages"])
                with pytest.raises(NousError) as failure:
                    await runtime.app.call(
                        MODEL_RUN,
                        RunModel(
                            task="read",
                            sources=(InlineText(text="Source"),),
                            output_schema=schema_for("Bob", "B"),
                        ),
                    )
                assert failure.value.code == "model.output_invalid"

        asyncio.run(scenario())


def test_task_contract_ignores_nested_schema_help_but_keeps_business_fields() -> None:
    from pydantic import Field, create_model

    from nous.models.tasks import TaskDefinition

    def task(help_text: str, *, min_length: int = 1) -> TaskDefinition:
        nested = create_model(
            "Nested",
            title=(str, Field(..., description=help_text, min_length=min_length)),
            description=(str, Field(..., description=help_text)),
            examples=(list[str], Field(..., description=help_text)),
        )
        output = create_model("Output", child=(nested, Field(..., description=help_text)))
        return TaskDefinition("nested-contract", output_type=output)

    before = task("Before")
    help_only = task("After")
    changed = task("After", min_length=2)
    assert before.interpretation_contract() == help_only.interpretation_contract()
    assert help_only.output_type is not None
    assert before.supports_schema(help_only.output_type.model_json_schema())
    assert before.interpretation_contract() != changed.interpretation_contract()
    assert before.output_type is not None and changed.output_type is not None
