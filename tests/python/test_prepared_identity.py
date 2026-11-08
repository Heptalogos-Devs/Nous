"""Prepared requests have durable semantic identity, independent of preview metadata."""

import asyncio
import json
from collections.abc import Generator
from dataclasses import replace
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from unittest.mock import patch

import pytest
from model_support import configure_models, model_server
from pydantic_ai.messages import BinaryContent, ModelRequest, UserPromptPart

from nous.common.errors import NousError
from nous.models.generation.contracts import ProducedTask
from nous.models.requests import PreparedGeneration
from nous.runtime.bootstrap import open_instance


class _Handler(BaseHTTPRequestHandler):
    requests: list[dict[str, object]] = []

    def do_POST(self) -> None:
        self.requests.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
        payload = json.dumps(
            {
                "id": "identity-test",
                "object": "chat.completion",
                "created": 1700000000,
                "model": "local-model",
                "choices": [
                    {
                        "index": 0,
                        "message": {"role": "assistant", "content": "Successful output."},
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


@pytest.fixture
def instance(tmp_path: Path) -> Generator[Path]:
    _Handler.requests = []
    path = tmp_path / "instance"
    with model_server(_Handler) as server:
        configure_models(path, server)
        yield path


def test_identical_prepared_request_survives_configuration_change_and_reopen(
    instance: Path,
) -> None:
    async def scenario() -> None:
        async with open_instance(instance, environment={}) as runtime:
            snapshot = runtime.configuration.snapshot
            source = await runtime.artifacts.ingest("Prepared identity source")
            selected = await runtime.generation.accept_inputs(source, snapshot)

            async def prepare() -> PreparedGeneration:
                current = runtime.configuration.snapshot
                plan = await runtime.generation.prepare_input("read", selected, current)
                prepared = await runtime.generation.prepare_group(
                    plan.frozen, plan.batches[0], current
                )
                assert isinstance(prepared, PreparedGeneration)
                return prepared

            before = await prepare()
            keys = (("implicit", None), ("explicit", "caller-semantic-identity"))
            first: dict[str, ProducedTask] = {}
            for suffix, identity in keys:
                first[suffix] = await runtime.generation.produce_prepared(
                    before, producer_key="stable-" + suffix, request_identity=identity
                )
            await runtime.configuration.update({"logging.level": "INFO"})
            after = await prepare()
            assert before.snapshot.version != after.snapshot.version
            for suffix, identity in keys:
                repeated = await runtime.generation.produce_prepared(
                    after, producer_key="stable-" + suffix, request_identity=identity
                )
                assert repeated.outputs["output"].data == first[suffix].outputs["output"].data
                await runtime.generation.produce_prepared(
                    after, producer_key="after-reopen-" + suffix, request_identity=identity
                )
            original_outputs = runtime.results.prepare_outputs

            async def fail_response(outputs: object) -> object:
                if isinstance(outputs, dict) and {"response", "computation"} <= outputs.keys():
                    raise OSError("injected publication failure")
                return await original_outputs(outputs)  # type: ignore[arg-type]

            with patch.object(runtime.results, "prepare_outputs", side_effect=fail_response):
                for suffix, identity in keys:
                    with pytest.raises(OSError):
                        await runtime.generation.produce_prepared(
                            after,
                            producer_key="pending-reopen-" + suffix,
                            request_identity=identity,
                        )
            for suffix, identity in keys:
                with pytest.raises(NousError) as conflict:
                    await runtime.generation.produce_prepared(
                        replace(after, coverage={"mode": "different"}),
                        producer_key="pending-reopen-" + suffix,
                        request_identity=identity,
                    )
                assert conflict.value.code == "content.producer_conflict"
            assert len(_Handler.requests) == 6
        async with open_instance(instance, environment={}) as runtime:
            snapshot = runtime.configuration.snapshot
            assert snapshot.version != after.snapshot.version
            plan = await runtime.generation.prepare_input("read", selected, snapshot)
            restored = await runtime.generation.prepare_group(
                plan.frozen, plan.batches[0], snapshot
            )
            assert isinstance(restored, PreparedGeneration)
            for suffix, identity in keys:
                accepted = await runtime.generation.produce_prepared(
                    restored, producer_key="after-reopen-" + suffix, request_identity=identity
                )
                pending = await runtime.generation.produce_prepared(
                    restored, producer_key="pending-reopen-" + suffix, request_identity=identity
                )
                assert accepted.outputs["output"].data == pending.outputs["output"].data
            assert len(_Handler.requests) == 6

    asyncio.run(scenario())


def test_actual_joint_and_per_batch_coverage_cannot_share_a_key(instance: Path) -> None:
    async def scenario() -> None:
        async with open_instance(instance, environment={}) as runtime:
            snapshot = runtime.configuration.snapshot
            source = await runtime.artifacts.ingest("same text source")
            selected = await runtime.generation.accept_inputs(source, snapshot)
            joint_plan = await runtime.generation.prepare_input("read", selected, snapshot)
            batch_plan = await runtime.generation.prepare_input(
                "read", selected, snapshot, per_batch=True
            )
            joint = await runtime.generation.prepare_group(
                joint_plan.frozen, joint_plan.batches[0], snapshot
            )
            batch = await runtime.generation.prepare_group(
                batch_plan.frozen, batch_plan.batches[0], snapshot
            )
            assert isinstance(joint, PreparedGeneration)
            assert isinstance(batch, PreparedGeneration)
            assert joint.coverage != batch.coverage
            assert joint.to_dict() == batch.to_dict()
            for supplied in (False, True):
                key = "supplied" if supplied else "automatic"
                identity = "same" if supplied else None
                await runtime.generation.produce_prepared(
                    joint, producer_key=key, request_identity=identity
                )
                with pytest.raises(NousError) as conflict:
                    await runtime.generation.produce_prepared(
                        batch, producer_key=key, request_identity=identity
                    )
                assert conflict.value.code == "content.producer_conflict"
            assert len(_Handler.requests) == 2

    asyncio.run(scenario())


def test_actual_equal_length_media_bytes_cannot_share_a_key(instance: Path) -> None:
    async def scenario() -> None:
        async with open_instance(instance, environment={}) as runtime:
            await runtime.configuration.update(
                {
                    "models.reader.capabilities": ["text", "stream", "structured_output", "image"],
                    "models.reader.input_formats": ["text/plain", "image/png"],
                }
            )
            snapshot = runtime.configuration.snapshot
            source = await runtime.artifacts.ingest("same text source")
            selected = await runtime.generation.accept_inputs(source, snapshot)
            plan = await runtime.generation.prepare_input("read", selected, snapshot)
            prepared = await runtime.generation.prepare_group(
                plan.frozen, plan.batches[0], snapshot
            )
            assert isinstance(prepared, PreparedGeneration)

            def with_media(data: bytes) -> PreparedGeneration:
                messages = list(prepared.compiled.messages)
                message = messages[-1]
                assert isinstance(message, ModelRequest)
                part = message.parts[0]
                assert isinstance(part, UserPromptPart) and not isinstance(part.content, str)
                updated = replace(
                    part, content=[*part.content, BinaryContent(data=data, media_type="image/png")]
                )
                messages[-1] = replace(message, parts=[updated])
                return replace(
                    prepared, compiled=replace(prepared.compiled, messages=tuple(messages))
                )

            first = with_media(b"first-image")
            second = with_media(b"other-image")
            assert first.to_dict() == second.to_dict()
            await runtime.generation.produce_prepared(first, producer_key="media")
            with pytest.raises(NousError) as conflict:
                await runtime.generation.produce_prepared(second, producer_key="media")
            assert conflict.value.code == "content.producer_conflict"
            assert len(_Handler.requests) == 1

    asyncio.run(scenario())


def test_identity_tracks_execution_and_recovery_facts_but_not_display(instance: Path) -> None:
    async def scenario() -> None:
        async with open_instance(instance, environment={}) as runtime:
            snapshot = runtime.configuration.snapshot
            source = await runtime.artifacts.ingest("identity contract source")
            selected = await runtime.generation.accept_inputs(source, snapshot)
            plan = await runtime.generation.prepare_input("read", selected, snapshot)
            prepared = await runtime.generation.prepare_group(
                plan.frozen, plan.batches[0], snapshot
            )
            assert isinstance(prepared, PreparedGeneration)
            await runtime.generation.produce_prepared(prepared, producer_key="contract")
            display_only = replace(
                prepared,
                compiled=replace(
                    prepared.compiled,
                    resource=replace(
                        prepared.compiled.resource, documentation_signature="changed-help"
                    ),
                ),
            )
            assert display_only.to_dict() != prepared.to_dict()
            await runtime.generation.produce_prepared(display_only, producer_key="contract")
            changed_model = prepared.routes[0].model.model_copy(
                update={"model_name": "different-model"}
            )
            changed_route = replace(prepared.routes[0], model=changed_model)
            variants = (
                replace(prepared, max_output_tokens=prepared.max_output_tokens + 1),
                replace(prepared, routes=(changed_route, *prepared.routes[1:])),
                replace(prepared, compiled=replace(prepared.compiled, schema={"type": "string"})),
                replace(prepared, stage_refs=("different-stage",)),
            )
            for changed in variants:
                with pytest.raises(NousError) as conflict:
                    await runtime.generation.produce_prepared(changed, producer_key="contract")
                assert conflict.value.code == "content.producer_conflict"
            with pytest.raises(NousError) as conflict:
                await runtime.generation.produce_prepared(
                    prepared, producer_key="contract", selected_inputs=selected
                )
            assert conflict.value.code == "content.producer_conflict"
            assert len(_Handler.requests) == 1

    asyncio.run(scenario())


def test_keyed_deferred_route_requires_a_stable_caller_identity(instance: Path) -> None:
    from nous.models.configuration import ModelRoute

    async def scenario() -> None:
        async with open_instance(instance, environment={}) as runtime:
            snapshot = runtime.configuration.snapshot
            source = await runtime.artifacts.ingest("identity contract source")
            selected = await runtime.generation.accept_inputs(source, snapshot)
            plan = await runtime.generation.prepare_input("read", selected, snapshot)
            prepared = await runtime.generation.prepare_group(
                plan.frozen, plan.batches[0], snapshot
            )
            assert isinstance(prepared, PreparedGeneration)
            fallback = replace(prepared.routes[0], model_id="fallback")
            prepared = replace(prepared, routes=(*prepared.routes, fallback))

            async def prepare_route(route: ModelRoute) -> PreparedGeneration:
                return prepared

            with pytest.raises(NousError) as missing:
                await runtime.generation.produce_prepared(
                    prepared, producer_key="deferred", prepare_route=prepare_route
                )
            assert missing.value.code == "model.prepared_identity_required"
            assert not _Handler.requests
            for _ in range(2):
                result = await runtime.generation.produce_prepared(
                    prepared,
                    producer_key="deferred",
                    prepare_route=prepare_route,
                    request_identity="same-deferred-input",
                )
                assert result.outputs["output"].data == b"Successful output."
            with pytest.raises(NousError) as conflict:
                await runtime.generation.produce_prepared(
                    prepared,
                    producer_key="deferred",
                    prepare_route=prepare_route,
                    request_identity="different-deferred-input",
                )
            assert conflict.value.code == "content.producer_conflict"
            assert len(_Handler.requests) == 1
            await runtime.generation.run_prepared(prepared, prepare_route=prepare_route)
            assert len(_Handler.requests) == 2

    asyncio.run(scenario())


def test_real_model_preset_change_conflicts_before_another_request(instance: Path) -> None:
    async def scenario() -> None:
        async with open_instance(instance, environment={}) as runtime:
            snapshot = runtime.configuration.snapshot
            source = await runtime.artifacts.ingest("identity contract source")
            selected = await runtime.generation.accept_inputs(source, snapshot)
            plan = await runtime.generation.prepare_input("read", selected, snapshot)
            original = await runtime.generation.prepare_group(
                plan.frozen, plan.batches[0], snapshot
            )
            assert isinstance(original, PreparedGeneration)
            await runtime.generation.produce_prepared(original, producer_key="preset")
            await runtime.configuration.update(
                {"presets.test-preset": {"temperature": 0.1}, "tasks.read.preset": "test-preset"}
            )
            changed_snapshot = runtime.configuration.snapshot
            plan = await runtime.generation.prepare_input("read", selected, changed_snapshot)
            changed = await runtime.generation.prepare_group(
                plan.frozen, plan.batches[0], changed_snapshot
            )
            assert isinstance(changed, PreparedGeneration)
            assert original.routes[0].preset != changed.routes[0].preset
            with pytest.raises(NousError) as conflict:
                await runtime.generation.produce_prepared(changed, producer_key="preset")
            assert conflict.value.code == "content.producer_conflict"
            assert len(_Handler.requests) == 1

    asyncio.run(scenario())


def test_keyed_custom_validator_requires_a_stable_caller_identity(instance: Path) -> None:
    from pydantic import JsonValue

    from nous.models.contracts import ReadingInput

    async def scenario() -> None:
        async with open_instance(instance, environment={}) as runtime:
            snapshot = runtime.configuration.snapshot
            source = await runtime.artifacts.ingest("identity contract source")
            selected = await runtime.generation.accept_inputs(source, snapshot)
            plan = await runtime.generation.prepare_input("read", selected, snapshot)
            prepared = await runtime.generation.prepare_group(
                plan.frozen, plan.batches[0], snapshot
            )
            assert isinstance(prepared, PreparedGeneration)

            def custom_validator(value: JsonValue, reading: ReadingInput) -> JsonValue:
                return value

            prepared = replace(
                prepared, compiled=replace(prepared.compiled, validate_output=custom_validator)
            )
            with pytest.raises(NousError) as missing:
                await runtime.generation.produce_prepared(prepared, producer_key="custom-validator")
            assert missing.value.code == "model.prepared_identity_required"
            for _ in range(2):
                await runtime.generation.produce_prepared(
                    prepared, producer_key="custom-validator", request_identity="validator-v1"
                )
            with pytest.raises(NousError) as conflict:
                await runtime.generation.produce_prepared(
                    prepared, producer_key="custom-validator", request_identity="validator-v2"
                )
            assert conflict.value.code == "content.producer_conflict"
            assert len(_Handler.requests) == 1

    asyncio.run(scenario())


def test_model_response_identity_uses_provider_binding_not_created_timestamp() -> None:
    from pydantic_ai.messages import ModelResponse, TextPart

    from nous.models.generation.identity import _message_value  # pyright: ignore[reportPrivateUsage]

    first = ModelResponse(parts=[TextPart("same")], provider_name="openai", canonical_system="a")
    same = ModelResponse(parts=[TextPart("same")], provider_name="openai", canonical_system="a")
    different = replace(first, canonical_system="b")
    assert _message_value(first) == _message_value(same)
    assert _message_value(first) != _message_value(different)
