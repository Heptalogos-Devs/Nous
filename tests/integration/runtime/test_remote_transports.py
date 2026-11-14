"""Remote transports share operations, content, authentication and cancellation."""

import asyncio
import base64
import json
from pathlib import Path
from typing import cast

import grpc
import httpx2
import pytest
from google.protobuf.wrappers_pb2 import BytesValue
from mcp import Client
from mcp.client.streamable_http import streamable_http_client
from mcp.types import TextContent
from pydantic import BaseModel

from nous.application.contracts import (
    BoundOperation,
    CallContext,
    EventEnvelope,
    Progress,
    Saved,
    define_operation,
)
from nous.application.dispatch import Application
from nous.cognition.writing.contracts import AddNote, AfterWrite, ListRecords
from nous.cognition.writing.operations import ADD_NOTE, LIST_RECORDS
from nous.content.contracts import InlineText
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.transports.grpc import SERVICE, serve_grpc
from nous.transports.http import serve_http
from nous.transports.mcp import serve_mcp

TOKEN = "test-only-token"


def test_remote_operations_and_content_round_trip(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            note = await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(text="A precise shared source."),
                    remember=True,
                    after=AfterWrite(annotation="skip"),
                ),
            )
            ref = note.commit.revision_ref.identifier
            headers = {"Authorization": f"Bearer {TOKEN}"}
            async with (
                serve_http(runtime.app, runtime.artifacts, token=TOKEN) as http,
                serve_mcp(runtime.app, runtime.artifacts, token=TOKEN) as mcp,
                serve_grpc(runtime.app, runtime.artifacts, token=TOKEN) as rpc,
            ):
                async with httpx2.AsyncClient(base_url=f"http://{http.host}:{http.port}") as client:
                    assert (await client.get("/operations")).status_code == 401
                    response = await client.post(
                        "/calls",
                        headers=headers,
                        json={"operation": "materials.read", "request": {"ref": ref}},
                    )
                    response.raise_for_status()
                    expected = response.json()["value"]
                    assert expected["selections"][0]["body_text"] == "A precise shared source."
                    selected_request = {
                        "operation": "materials.read",
                        "request": {"ref": ref},
                        "select": ["/material/kind", "/selections/0/body_text"],
                    }
                    selected = await client.post("/calls", headers=headers, json=selected_request)
                    assert selected.json()["value"] == {
                        "/material/kind": expected["material"]["kind"],
                        "/selections/0/body_text": "A precise shared source.",
                    }
                    streamed = await client.post(
                        "/calls/stream", headers=headers, json=selected_request
                    )
                    messages = [
                        json.loads(line.removeprefix("data: "))
                        for line in streamed.text.splitlines()
                        if line.startswith("data: ")
                    ]
                    assert messages[-1]["kind"] == "result"
                    assert messages[-1]["value"] == selected.json()["value"]
                    missing = await client.post(
                        "/calls", headers=headers, json={**selected_request, "select": ["/missing"]}
                    )
                    assert missing.status_code == 400
                    assert missing.json()["error"]["code"] == "input.invalid_selection"
                    before = len(await runtime.app.call(LIST_RECORDS, ListRecords()))
                    write = {
                        "operation": "records.add_note",
                        "request": {
                            "body": {"kind": "text", "text": "A selected write."},
                            "after": {"annotation": "skip"},
                        },
                    }
                    malformed = await client.post(
                        "/calls", headers=headers, json={**write, "select": ["not-a-pointer"]}
                    )
                    assert malformed.status_code == 400
                    assert len(await runtime.app.call(LIST_RECORDS, ListRecords())) == before
                    missing_after_write = await client.post(
                        "/calls", headers=headers, json={**write, "select": ["/missing"]}
                    )
                    assert missing_after_write.status_code == 400
                    error = missing_after_write.json()["error"]
                    assert error["code"] == "input.invalid_selection"
                    assert error["details"]["completed_refs"]
                    assert len(await runtime.app.call(LIST_RECORDS, ListRecords())) == before + 1
                    record_request = {"operation": "records.read", "request": {"target": ref}}
                    record = await client.post("/calls", headers=headers, json=record_request)
                    full_record = record.json()["value"]
                    selected_record = await client.post(
                        "/calls",
                        headers=headers,
                        json={**record_request, "select": ["/body_selection/artifact/identifier"]},
                    )
                    assert selected_record.json()["value"] == {
                        "/body_selection/artifact/identifier": full_record["body_selection"][
                            "artifact"
                        ]["identifier"]
                    }
                    uploaded = await client.post(
                        "/content?name=sample.txt", headers=headers, content=b"exact bytes"
                    )
                    uploaded.raise_for_status()
                    content_ref = uploaded.json()["value"]["ref"]["identifier"]
                    download = await client.get("/content/" + content_ref, headers=headers)
                    assert download.content == b"exact bytes"

                async with grpc.aio.insecure_channel(f"{rpc.host}:{rpc.port}") as channel:
                    call = channel.unary_unary(
                        f"/{SERVICE}/Call",
                        request_serializer=BytesValue.SerializeToString,
                        response_deserializer=BytesValue.FromString,
                    )
                    request = BytesValue(
                        value=json.dumps(
                            {"operation": "materials.read", "request": {"ref": ref}}
                        ).encode()
                    )
                    result = await call(request, metadata=(("authorization", f"Bearer {TOKEN}"),))
                    assert json.loads(result.value)["value"] == expected
                    selected_request = BytesValue(
                        value=json.dumps(
                            {
                                "operation": "materials.read",
                                "request": {"ref": ref},
                                "select": ["/selections/0/body_text"],
                            }
                        ).encode()
                    )
                    result = await call(
                        selected_request, metadata=(("authorization", f"Bearer {TOKEN}"),)
                    )
                    assert json.loads(result.value)["value"] == {
                        "/selections/0/body_text": "A precise shared source."
                    }
                    streaming = channel.unary_stream(
                        f"/{SERVICE}/StreamCall",
                        request_serializer=BytesValue.SerializeToString,
                        response_deserializer=BytesValue.FromString,
                    )
                    events = [
                        json.loads(item.value)
                        async for item in streaming(
                            selected_request, metadata=(("authorization", f"Bearer {TOKEN}"),)
                        )
                    ]
                    assert events[-1]["kind"] == "result"
                    assert events[-1]["value"] == {
                        "/selections/0/body_text": "A precise shared source."
                    }
                    write_request = BytesValue(
                        value=json.dumps(
                            {
                                "operation": "records.add_note",
                                "request": {
                                    "body": {"kind": "text", "text": "Streamed write."},
                                    "after": {"annotation": "skip"},
                                },
                                "select": ["/commit/revision_ref"],
                            }
                        ).encode()
                    )
                    events = [
                        json.loads(item.value)
                        async for item in streaming(
                            write_request, metadata=(("authorization", f"Bearer {TOKEN}"),)
                        )
                    ]
                    assert any(
                        item["kind"] == "event" and item["value"]["event"]["kind"] == "saved"
                        for item in events
                    )
                    assert events[-1]["value"]["/commit/revision_ref"]["kind"] == "record_revision"
                    with pytest.raises(grpc.aio.AioRpcError) as error:
                        await call(request)
                    assert error.value.code() == grpc.StatusCode.UNAUTHENTICATED

                async with httpx2.AsyncClient(headers=headers) as client:
                    transport = streamable_http_client(
                        f"http://{mcp.host}:{mcp.port}/mcp", http_client=client
                    )
                    async with Client(transport) as session:
                        reply = await session.call_tool("materials.read", {"ref": ref})
                        assert not reply.is_error
                        assert reply.structured_content["value"] == expected
                        assert isinstance(reply.content[0], TextContent)
                        assert "A precise shared source." in reply.content[0].text
                        assert '"body_text"' not in reply.content[0].text
                        selected = await session.call_tool(
                            "materials.read", {"ref": ref, "_select": ["/selections/0/body_text"]}
                        )
                        assert not selected.is_error
                        assert selected.structured_content["value"] == {
                            "/selections/0/body_text": "A precise shared source."
                        }
                        invalid = await session.call_tool(
                            "materials.read", {"ref": ref, "_select": ["/missing"]}
                        )
                        assert invalid.is_error
                        full = await session.call_tool("records.read", {"target": ref})
                        assert not full.is_error and full.structured_content["value"] == full_record
                        selected_record = await session.call_tool(
                            "records.read",
                            {"target": ref, "_select": ["/body_selection/artifact/identifier"]},
                        )
                        assert not selected_record.is_error
                        assert selected_record.structured_content["value"] == {
                            "/body_selection/artifact/identifier": full_record["body_selection"][
                                "artifact"
                            ]["identifier"]
                        }
                        found = await session.call_tool(
                            "retrieval.query", {"text": "precise shared", "save": True}
                        )
                        assert not found.is_error
                        assert isinstance(found.content[0], TextContent)
                        assert "A precise shared source." in found.content[0].text
                        query = found.structured_content["value"]
                        assert query["evidence"]["passages"]
                        evidence = await session.call_tool(
                            "materials.read",
                            {
                                "delivery": query["evidence_ref"]["identifier"],
                                "key": query["evidence"]["passages"][0]["key"],
                            },
                        )
                        assert not evidence.is_error
                        assert isinstance(evidence.content[0], TextContent)
                        assert "A precise shared source." in evidence.content[0].text
                        notifications: list[EventEnvelope] = []

                        async def on_progress(
                            progress: float, total: float | None, message: str | None
                        ) -> None:
                            if message is not None:
                                notifications.append(EventEnvelope.model_validate_json(message))

                        created = await session.call_tool(
                            "records.add_note",
                            {
                                "body": {"kind": "text", "text": "Created through MCP."},
                                "after": {"annotation": "skip"},
                            },
                            progress_callback=on_progress,
                        )
                        assert not created.is_error
                        assert any(isinstance(item.event, Saved) for item in notifications)
                        payload = b"a" * (512 * 1024)
                        uploaded = await session.call_tool(
                            "content.upload",
                            {"name": "mcp.bin", "data": base64.b64encode(payload).decode()},
                        )
                        assert not uploaded.is_error
                        artifact = uploaded.structured_content["value"]["ref"]["identifier"]
                        downloaded = await session.call_tool("content.download", {"ref": artifact})
                        assert not downloaded.is_error
                        assert (
                            base64.b64decode(downloaded.structured_content["value"]["data"])
                            == payload
                        )

    asyncio.run(scenario())


def test_remote_disconnect_cancels_active_operation(tmp_path: Path) -> None:
    class Empty(BaseModel):
        pass

    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        started, exited = asyncio.Event(), asyncio.Event()
        operation = define_operation("test.wait", request=Empty, result=str)

        async def wait(request: Empty, call: CallContext) -> str:
            try:
                await call.events.emit(Progress(stage="waiting"))
                started.set()
                await asyncio.Event().wait()
            finally:
                exited.set()
            return "unreachable"

        async with open_instance(instance, environment={}) as runtime:
            app = Application(
                (cast(BoundOperation[object, object], BoundOperation(operation, wait)),),
                execution=runtime.execution,
                artifacts=runtime.artifacts,
                results=runtime.results,
                readers=runtime.artifacts.content.file_readers,
            )
            async with serve_http(app, runtime.artifacts, token=TOKEN) as http:
                async with httpx2.AsyncClient(
                    base_url=f"http://{http.host}:{http.port}",
                    headers={"Authorization": f"Bearer {TOKEN}"},
                ) as client:
                    task = asyncio.create_task(
                        client.post("/calls", json={"operation": "test.wait", "request": {}})
                    )
                    await asyncio.wait_for(started.wait(), 5)
                    task.cancel()
                    await asyncio.gather(task, return_exceptions=True)
                    await asyncio.wait_for(exited.wait(), 5)
            started.clear()
            exited.clear()
            async with serve_grpc(app, runtime.artifacts, token=TOKEN) as rpc:
                async with grpc.aio.insecure_channel(f"{rpc.host}:{rpc.port}") as channel:
                    call = channel.unary_stream(
                        f"/{SERVICE}/StreamCall",
                        request_serializer=BytesValue.SerializeToString,
                        response_deserializer=BytesValue.FromString,
                    )
                    request = BytesValue(value=b'{"operation":"test.wait","request":{}}')
                    stream = call(request, metadata=(("authorization", f"Bearer {TOKEN}"),))
                    await asyncio.wait_for(started.wait(), 5)
                    stream.cancel()
                    await asyncio.wait_for(exited.wait(), 5)

    asyncio.run(scenario())
