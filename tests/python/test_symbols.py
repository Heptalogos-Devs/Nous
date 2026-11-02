"""Explicit symbol inference retains its paid result and loses withdrawn source text."""

import asyncio
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import httpx
import pytest
from nous_sdk import NousClient, NousError
from nous_sdk.contracts import (
    AdmitFiles,
    ImportDocument,
    MediaReference,
    SourceCommand,
)
from nous.platform.call_context import CallContext
from nous_sdk.contracts.context import ContextPrepare, ContextUsage
from nous_sdk.contracts.social import RecoverSymbolResult, SymbolSelection
from nous_sdk.transport import ClientOptions

from nous.platform.configuration import ModelRoute, Settings
from nous.platform.configuration.models import ModelConfiguration
from nous.transports.http import create_app


def test_semantic_symbol_sdk_context_replay_and_source_withdrawal(tmp_path, monkeypatch):
    received = []

    class Provider(BaseHTTPRequestHandler):
        def log_message(self, format, *args):
            pass

        def do_POST(self):
            received.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            decision = {
                "matches": [],
                "ambiguous": False,
                "local_interpretation": {
                    "content_description": "The supplied project README uses Nous as its name.",
                    "meaning": "Project name",
                    "expression": "Nous",
                    "emotions": [],
                    "intentions": "Identify the project name without altering this item.",
                    "confidence": None,
                    "guidance": "Use the project name.",
                    "alternatives": [],
                    "unknown_aspects": [],
                    "source_keys": ["input"],
                },
            }
            body = json.dumps(
                {
                    "id": "local-symbol-contract",
                    "object": "chat.completion",
                    "created": 0,
                    "model": "local-symbol-provider",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": json.dumps(decision)},
                            "finish_reason": "stop",
                        }
                    ],
                    "usage": {"prompt_tokens": 20, "completion_tokens": 20, "total_tokens": 40},
                }
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    monkeypatch.setenv("NOUS_SYMBOL_LOCAL_KEY", "local-test-key")

    async def scenario():
        settings = Settings(
            instance=tmp_path / "instance",
            model=ModelRoute(
                base_url=f"http://127.0.0.1:{server.server_port}/v1",
                model="local-symbol-provider",
                credential_env="NOUS_SYMBOL_LOCAL_KEY",
            ),
        )
        web = create_app(settings)
        async with web.router.lifespan_context(web):
            async with NousClient(
                "http://test",
                credential_file=settings.token_file,
                options=ClientOptions(language="zh-CN"),
            ) as client:
                await client.transport.http.aclose()
                client.transport.http = httpx.AsyncClient(
                    transport=httpx.ASGITransport(app=web), base_url="http://test"
                )
                namespace = await client.create_namespace("Project")
                subject = await client.create_subject(
                    namespace.id, name="Project", capabilities=("memory", "social")
                )
                app = web.state.application
                ctx = CallContext(
                    caller_id="instance-admin",
                    namespace_id=namespace.id,
                    subject_id=subject.id,
                    credential_source_ids=None,
                )
                text = (await asyncio.to_thread(Path("README.md").read_text, encoding="utf-8"))[
                    :600
                ]
                imported = await app.imports.admit_files(
                    ctx,
                    AdmitFiles(
                        inputs=(ImportDocument(text=text, filename="README.md"),),
                        llm_enabled=False,
                        vectors="disabled",
                    ),
                )
                reference = imported.outputs[0].cognition[0]
                carrier = MediaReference(
                    kind="material",
                    id=imported.catalog.units[0].material_id,
                    observation_id=imported.outputs[0].observation_id,
                )
                command = SymbolSelection(
                    expression="Nous",
                    messages=(reference,),
                    message_sources=(carrier,),
                    semantic="required",
                    invocation_id="read-project-symbol",
                )
                free = await client.understand_symbol(
                    namespace.id, subject.id, command.model_copy(update={"semantic": "off"})
                )
                assert free.unknown and free.local_interpretation is None and not received
                result = await client.understand_symbol(namespace.id, subject.id, command)
                assert (
                    result.unknown and result.semantic_status == "used" and result.producer_call_id
                )
                assert any(
                    item.reference is not None and item.reference.reference == reference
                    for item in result.local_interpretation.sources
                )
                assert await client.understand_symbol(namespace.id, subject.id, command) == result
                assert len(received) == 1
                assert result.local_interpretation.intentions == (
                    "Identify the project name without altering this item.",
                )
                recovered = await client.recover_symbol_result(
                    namespace.id,
                    subject.id,
                    RecoverSymbolResult(selection=command, call_id=result.producer_call_id),
                )
                assert recovered.reparsed and recovered.producer_call_status == "succeeded"
                assert (
                    recovered.local_interpretation == result.local_interpretation
                    and len(received) == 1
                )
                wire = json.dumps(received[0], ensure_ascii=False)
                wire_text = "\n".join(
                    part.get("text", "")
                    for message in received[0]["messages"]
                    for part in message["content"]
                    if isinstance(part, dict)
                )
                assert text[:80] in wire_text and "media0" in wire and "m0" in wire
                assert "解释语言：zh-CN" in wire_text
                prepared = await app.contexts.prepare(
                    ctx,
                    ContextPrepare(
                        session_id="project-symbol",
                        consumer_id="default",
                        instance_id="sdk",
                        usage=ContextUsage(
                            purpose="reply",
                            symbols_mode="express",
                            symbols=command.model_copy(
                                update={"invocation_id": "express-project-symbol"}
                            ),
                        ),
                    ),
                )
                local_segment = next(
                    segment
                    for segment in prepared.response.projection.segments
                    if segment.provenance.get("supply") == "local_symbol_interpretation"
                )
                assert any(
                    item.kind == "media" and item.reference == carrier
                    for item in local_segment.dependencies
                )
                assert len(received) == 2
                configuration = ModelConfiguration.model_validate(
                    (await app.configuration.effective(ctx, "models"))["value"]
                )
                configuration.models["generation"] = configuration.models["generation"].model_copy(
                    update={"name": ""}
                )
                await app.configuration.set(ctx, "models", configuration)
                optional = await client.understand_symbol(
                    namespace.id,
                    subject.id,
                    command.model_copy(
                        update={"semantic": "optional", "invocation_id": "optional-symbol"}
                    ),
                )
                assert optional.semantic_status == "unavailable" and len(received) == 2
                with pytest.raises(NousError) as required:
                    await client.understand_symbol(
                        namespace.id,
                        subject.id,
                        command.model_copy(update={"invocation_id": "required-unavailable"}),
                    )
                assert required.value.code == "model_unavailable"
                await app.sources.trash(
                    ctx,
                    SourceCommand(
                        observation_ids=(imported.outputs[0].observation_id,),
                        reason="Withdraw README source",
                    ),
                )
                await app.sources.purge(
                    ctx,
                    SourceCommand(
                        observation_ids=(imported.outputs[0].observation_id,),
                        force=True,
                        reason="Erase README source",
                    ),
                )
                with pytest.raises(NousError) as purged:
                    await client.understand_symbol(namespace.id, subject.id, command)
                assert purged.value.code == "operation_result_purged"
                assert len(received) == 2

    try:
        asyncio.run(scenario())
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)
