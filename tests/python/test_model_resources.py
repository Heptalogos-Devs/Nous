"""Current model resources, compiled provider messages, and immutable completed results."""

import asyncio
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from threading import Thread
from uuid import uuid4

import pytest
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.prompt_compilation import PromptMessage
from pydantic import BaseModel

from nous.application import Application
from nous.platform.configuration import Settings
from nous.platform.configuration.models import ModelConfiguration
from nous.platform.models.roles import RoleDefinition


class SelectionInput(BaseModel):
    question: str
    passages: tuple[str, ...]


def test_compiled_messages_primitive_output_and_resource_budget(tmp_path, monkeypatch):
    received = []
    readme = Path("README.md").read_text(encoding="utf-8")[:1000]

    class Provider(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            received.append(body)
            result = json.dumps(
                {
                    "id": f"wire-{len(received)}",
                    "object": "chat.completion",
                    "created": 1,
                    "model": body["model"],
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": '{"response":[0]}'},
                            "finish_reason": "stop",
                        }
                    ],
                    "usage": {"prompt_tokens": 20, "completion_tokens": 5, "total_tokens": 25},
                }
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(result)))
            self.end_headers()
            self.wfile.write(result)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    thread = Thread(target=server.serve_forever, daemon=True)
    thread.start()
    monkeypatch.setenv("NOUS_WIRE_TEST_KEY", "local-protocol-fixture")

    async def scenario():
        app = Application(Settings(instance=tmp_path / "instance"))
        app.models.roles.register(
            RoleDefinition("wire_selection", "indices", SelectionInput, list[int], default=True)
        )
        await app.authority.initialize()
        await app.configuration.initialize()
        ctx = CallContext(caller_id="owner", namespace_id=str(uuid4()), credential_source_ids=None)
        try:
            await app.identity.create_namespace(
                ctx, "protocol", "create", namespace_id=ctx.namespace_id
            )
            await app.configuration.set(ctx, "network", {"trust_env": False})
            await app.prompts.save(
                ctx,
                "wire-selection",
                "选择相关段落，返回编号。\n<!-- input -->\n{{ question }}\n{{ passages | tojson }}",
                "prompt",
            )
            await app.configuration.set(
                ctx,
                "models",
                {
                    "providers": {
                        "wire": {
                            "base_url": f"http://127.0.0.1:{server.server_port}/v1",
                            "credential_env": "NOUS_WIRE_TEST_KEY",
                        }
                    },
                    "catalog": {
                        "wire": {
                            "provider": "wire",
                            "actual_name": "gpt-4.1-mini",
                            "operations": ["generation"],
                            "max_requests_per_day": 1,
                        }
                    },
                    "presets": {"wire": {"model": "wire", "max_output_tokens": 128}},
                    "roles": {
                        role: {
                            "preset": "wire",
                            "purpose": "wire_selection",
                            "prompt_id": "wire-selection",
                            "budget": {"max_calls_per_day": 4},
                        }
                        for role in ("wire", "wire_other")
                    },
                },
            )
            variables = {
                "question": "软件提供什么能力？",
                "passages": [readme],
            }
            context = (PromptMessage(role="user", content="当前任务：阅读项目介绍。"),)
            generated = await app.models.generate(
                ctx,
                "wire",
                variables,
                list[int],
                invocation_id="selection",
                context_messages=context,
            )
            assert generated.output == [0]
            assert len(received) == 1
            request = received[0]
            assert request["messages"][0] == {
                "role": "system",
                "content": "选择相关段落，返回编号。",
            }
            assert any(item.get("content") == context[0].content for item in request["messages"])
            assert request["response_format"]["type"] == "json_schema"
            assert (
                request["response_format"]["json_schema"]["schema"]["properties"]["response"][
                    "type"
                ]
                == "array"
            )
            assert "tools" not in request
            await app.prompts.save(
                ctx,
                "wire-selection",
                "新的编辑说明。\n<!-- input -->\n{{ question }}\n{{ passages | tojson }}",
                "edit-prompt",
            )
            reused = await app.models.generate(
                ctx,
                "wire",
                variables,
                list[int],
                invocation_id="selection",
                context_messages=context,
            )
            assert (
                reused.call_id == generated.call_id and reused.output == [0] and len(received) == 1
            )
            with pytest.raises(DomainError) as exhausted:
                await app.models.generate(
                    ctx, "wire_other", variables, list[int], invocation_id="other"
                )
            assert exhausted.value.code == "model_budget_exhausted" and len(received) == 1
            resources = ModelConfiguration.model_validate(
                (await app.configuration.effective(ctx, "models"))["value"]
            )
            resources.catalog["wire"] = resources.catalog["wire"].model_copy(
                update={"max_requests_per_day": 2}
            )
            await app.configuration.set(
                ctx, "models", resources.model_dump(mode="json")
            )
            resumed = await app.models.generate(
                ctx, "wire_other", variables, list[int], invocation_id="other"
            )
            assert resumed.output == [0] and len(received) == 2
        finally:
            await app.close()

    try:
        asyncio.run(scenario())
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def test_toml_reload_scope_and_restart(tmp_path):
    async def scenario():
        app = Application(Settings(instance=tmp_path / "instance"))
        await app.authority.initialize()
        await app.configuration.initialize()
        ctx = CallContext(caller_id="owner", namespace_id=str(uuid4()), credential_source_ids=None)
        await app.identity.create_namespace(
            ctx, "settings", "create", namespace_id=ctx.namespace_id
        )
        try:
            await app.configuration.set(
                ctx, "growth", {"mode": "review", "max_snapshot_entries": 17}
            )
            await app.configuration.set(
                ctx, "growth", {"mode": "off"}, scope="namespace"
            )
            effective = (await app.configuration.effective(ctx, "growth"))["value"]
            assert effective["mode"] == "off" and effective["max_snapshot_entries"] == 17
            path = app.configuration.path(ctx, "namespace")
            path.write_text('[growth]\nmode="review"\n', encoding="utf-8")
            await app.configuration.reload(ctx, "namespace")
            assert (await app.configuration.effective(ctx, "growth"))["value"]["mode"] == "review"
        finally:
            await app.close()
        restored = Application(Settings(instance=tmp_path / "instance"))
        await restored.authority.initialize()
        await restored.configuration.initialize()
        try:
            result = (await restored.configuration.effective(ctx, "growth"))["value"]
            assert result["mode"] == "review" and result["max_snapshot_entries"] == 17
        finally:
            await restored.close()

    asyncio.run(scenario())
