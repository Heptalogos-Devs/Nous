"""Model protocol and retrieval through the real SDK and CLI."""

import asyncio
import json
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from typing import cast

import pytest
from model_support import configure_models, model_cli, model_server, persisted_task_results
from pydantic import JsonValue, TypeAdapter

from nous.application.configuration import open_configuration_operations


@pytest.mark.parametrize("streaming", [False, True], ids=["response", "stream"])
def test_same_model_preset_fallback_is_prepared_and_persisted(
    tmp_path: Path, streaming: bool
) -> None:
    submissions: list[dict[str, JsonValue]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            request = TypeAdapter(dict[str, JsonValue]).validate_json(
                self.rfile.read(int(self.headers["Content-Length"]))
            )
            submissions.append(request)
            status = 503 if len(submissions) == 1 else 200
            content_type = "application/json"
            base: dict[str, JsonValue] = {
                "id": "preset-response",
                "created": 1700000000,
                "model": request["model"],
            }
            if status == 503:
                payload = json.dumps({"error": {"message": "Primary unavailable"}}).encode()
            elif request.get("stream"):
                content_type = "text/event-stream"
                frames: list[dict[str, JsonValue]] = [
                    {
                        **base,
                        "object": "chat.completion.chunk",
                        "choices": [
                            {
                                "index": 0,
                                "delta": {"role": "assistant", "content": "Backup response."},
                                "finish_reason": None,
                            }
                        ],
                    },
                    {
                        **base,
                        "object": "chat.completion.chunk",
                        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                    },
                ]
                payload = (
                    "".join("data: " + json.dumps(frame) + "\n\n" for frame in frames)
                    + "data: [DONE]\n\n"
                ).encode()
            else:
                payload = json.dumps(
                    {
                        **base,
                        "object": "chat.completion",
                        "choices": [
                            {
                                "index": 0,
                                "message": {"role": "assistant", "content": "Backup response."},
                                "finish_reason": "stop",
                            }
                        ],
                    }
                ).encode()
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, format: str, *args: object) -> None:
            pass

    instance = tmp_path / "preset-fallback-instance"
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def configure() -> None:
            async with open_configuration_operations(instance, environment={}) as configuration:
                await configuration.update(
                    {
                        "presets.primary": {"temperature": 0.1, "max_output_tokens": 64},
                        "presets.backup": {"temperature": 0.7, "max_output_tokens": 128},
                        "tasks.read.preset": "primary",
                        "tasks.read.fallback": [{"model": "reader", "preset": "backup"}],
                    }
                )

        asyncio.run(configure())
        response = model_cli(
            instance,
            "tools",
            "model",
            "run",
            "--task",
            "read",
            "--text",
            "Use the backup preset if the primary request fails.",
            *(["--stream"] if streaming else []),
        )
    assert response.returncode == 0, (response.stdout, response.stderr)
    assert [request["model"] for request in submissions] == ["local-model", "local-model"]
    assert [request["temperature"] for request in submissions] == [0.1, 0.7]
    assert [request["max_completion_tokens"] for request in submissions] == [64, 128]
    if streaming:
        events = [json.loads(line) for line in response.stdout.splitlines()]
        assert any(event["event"] == "delta" for event in events)
        assert events[-1]["event"] == "completed"
        reference = events[-1]["result"]
    else:
        reference = json.loads(response.stdout)["results"][0]["ref"]["identifier"]
    restored = model_cli(instance, "tools", "result", "read", reference, "--details")
    assert restored.returncode == 0, restored.stderr
    result = json.loads(restored.stdout)
    assert result["outputs"][0]["view"]["body_text"] == "Backup response."
    assert result["parameters"]["attempts"] == 2
    assert result["request"]["max_output_tokens"] == 128
    assert result["request"]["prepared_attempt"]["parameters"]["temperature"] == 0.7
    assert result["request"]["prepared_attempt"]["max_output_tokens"] == 128
    assert result["request"]["prepared_attempt"]["attempt_number"] == 2
    assert [report["outcome"] for report in result["request"]["attempt_reports"]] == [
        "retry",
        "success",
    ]
    assert persisted_task_results(instance) == 1


@pytest.mark.parametrize("complete", [False, True], ids=["early-eof", "finished"])
def test_model_stream_requires_provider_completion_before_adoption(
    tmp_path: Path, complete: bool
) -> None:
    submissions: list[object] = []

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_POST(self) -> None:
            submissions.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Connection", "close")
            self.end_headers()
            base = {
                "id": "stream-response",
                "object": "chat.completion.chunk",
                "created": 1700000000,
                "model": "local-model",
            }
            frames: list[dict[str, object]] = [
                {
                    **base,
                    "choices": [
                        {
                            "index": 0,
                            "delta": {"role": "assistant", "content": "Visible response."},
                            "finish_reason": None,
                        }
                    ],
                }
            ]
            if complete:
                frames.append(
                    {
                        **base,
                        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                        "usage": {"prompt_tokens": 10, "completion_tokens": 3, "total_tokens": 13},
                    }
                )
            for frame in frames:
                self.wfile.write(("data: " + json.dumps(frame) + "\n\n").encode())
            if complete:
                self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
            self.close_connection = True

        def log_message(self, format: str, *args: object) -> None:
            pass

    instance = tmp_path / "stream-instance"
    with model_server(Handler) as server:
        configure_models(instance, server)
        response = model_cli(
            instance,
            "tools",
            "model",
            "run",
            "--task",
            "read",
            "--text",
            "Streaming source.",
            "--stream",
        )
    events = [json.loads(line) for line in response.stdout.splitlines()]
    assert len(submissions) == 1
    assert any(event["event"] == "delta" for event in events)
    if complete:
        assert response.returncode == 0, response.stderr
        assert events[-1]["event"] == "completed"
        restored = model_cli(instance, "tools", "result", "read", events[-1]["result"])
        assert restored.returncode == 0, restored.stderr
        assert json.loads(restored.stdout)["outputs"][0]["view"]["body_text"] == "Visible response."
        assert persisted_task_results(instance) == 1
    else:
        assert response.returncode != 0
        assert not any(event["event"] == "completed" for event in events)
        error = events[-1]["error"]
        assert error["details"]["partial_output_delivered"] is True
        assert "finish_reason" in error["details"]["provider_error"]["message"]
        original = model_cli(instance, "tools", "content", "read", error["details"]["artifacts"][0])
        assert original.returncode == 0, original.stderr
        assert json.loads(original.stdout)["text"] == "Streaming source."
        assert persisted_task_results(instance) == 0


@pytest.mark.parametrize("fallback", [False, True], ids=["repair", "repair-then-fallback"])
@pytest.mark.parametrize("output_mode", ["native", "tool"])
def test_model_structured_repair_and_fallback_survive_restart(
    tmp_path: Path, output_mode: str, fallback: bool
) -> None:
    submissions: list[dict[str, object]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            submissions.append(request)
            if fallback and len(submissions) == 2:
                body = json.dumps({"error": {"message": "Repair request unavailable"}}).encode()
                self.send_response(503)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            content = json.dumps({"answer": "invalid" if len(submissions) == 1 else 42})
            message: dict[str, object] = {"role": "assistant", "content": content}
            if output_mode == "tool":
                message = {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "id": f"call-{len(submissions)}",
                            "type": "function",
                            "function": {"name": "result", "arguments": content},
                        }
                    ],
                }
            body = json.dumps(
                {
                    "id": f"response-{len(submissions)}",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "local-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": message,
                            "finish_reason": "tool_calls" if output_mode == "tool" else "stop",
                        }
                    ],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 3, "total_tokens": 13},
                }
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format: str, *args: object) -> None:
            pass

    instance = tmp_path / "repair-instance"
    schema = tmp_path / "schema.json"
    schema.write_text(
        json.dumps(
            {
                "type": "object",
                "properties": {"answer": {"type": "integer"}},
                "required": ["answer"],
                "additionalProperties": False,
            }
        ),
        encoding="utf-8",
    )
    with model_server(Handler) as server:
        configure_models(instance, server, output_mode)
        if fallback:

            async def configure() -> None:
                async with open_configuration_operations(instance, environment={}) as configuration:
                    await configuration.update(
                        {
                            "model_execution.max_attempts": 3,
                            "presets.backup": {"temperature": 0.7},
                            "tasks.read.fallback": [{"model": "reader", "preset": "backup"}],
                        }
                    )

            asyncio.run(configure())
        response = model_cli(
            instance,
            "tools",
            "model",
            "run",
            "--task",
            "read",
            "--schema",
            str(schema),
            "--text",
            "The answer is 42.",
        )
    assert response.returncode == 0, (response.stdout, response.stderr)
    assert len(submissions) == (3 if fallback else 2)
    messages = cast(list[dict[str, object]], submissions[1]["messages"])
    assert messages[-1]["role"] == ("tool" if output_mode == "tool" else "user")
    if fallback:
        assert submissions[-1]["temperature"] == 0.7
        assert submissions[-1]["messages"] == submissions[0]["messages"]
    result = json.loads(response.stdout)["results"][0]
    assert result["parameters"]["attempts"] == len(submissions)
    assert result["parameters"]["usage"]["input_tokens"] == 20
    restored = model_cli(
        instance, "tools", "result", "read", result["ref"]["identifier"], "--details"
    )
    assert restored.returncode == 0, restored.stderr
    assert json.loads(json.loads(restored.stdout)["outputs"][0]["view"]["body_text"]) == {
        "answer": 42
    }
    assert persisted_task_results(instance) == 1


@pytest.mark.parametrize(
    "payload, accepted",
    [
        ({"data": None}, False),
        ({"data": [{"index": 0, "embedding": ["not-a-number", 0.5]}]}, False),
        ({"data": [{"index": 0, "embedding": [0.5, 0.25]}], "usage": []}, False),
        ({"data": [{"index": False, "embedding": [0.5, 0.25]}]}, False),
        ({"data": [{"index": 0, "embedding": [0.5, 0.25]}], "usage": None}, True),
    ],
    ids=[
        "missing-vectors",
        "nonnumeric-vector",
        "malformed-usage",
        "boolean-index",
        "unknown-usage",
    ],
)
def test_embedding_response_validation_and_adoption_survive_restart(
    tmp_path: Path, payload: dict[str, object], accepted: bool
) -> None:
    submissions: list[object] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            submissions.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            body = json.dumps({"object": "list", "model": "local-embedding", **payload}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format: str, *args: object) -> None:
            pass

    instance = tmp_path / "embedding-instance"
    with model_server(Handler) as server:
        configure_models(instance, server)
        response = model_cli(
            instance,
            "tools",
            "embedding",
            "run",
            "--space",
            "test-space",
            "--purpose",
            "document",
            "--text",
            "A vector source.",
        )
    assert len(submissions) == 1
    if accepted:
        assert response.returncode == 0, (response.stdout, response.stderr)
        result = json.loads(response.stdout)
        restored = model_cli(
            instance, "tools", "result", "read", result["results"][0]["identifier"], "--details"
        )
        assert restored.returncode == 0, restored.stderr
        assert json.loads(restored.stdout)["parameters"]["usage"] == {}
        vectors = result["rows"][0]["artifact"]["identifier"]
        export = tmp_path / "vectors.npy"
        exported = model_cli(
            instance, "tools", "content", "export", vectors, "--output", str(export)
        )
        assert exported.returncode == 0, exported.stderr
        import numpy as np

        assert np.array_equal(
            np.load(export, allow_pickle=False), np.array([[0.5, 0.25]], dtype=np.float32)
        )
        assert persisted_task_results(instance) == 1
        return
    assert response.returncode != 0
    error = json.loads(response.stdout)["error"]
    assert error["code"] == "embedding.invalid_response", (response.stdout, response.stderr)
    reopened = model_cli(instance, "runtime", "check")
    assert reopened.returncode == 0, reopened.stderr
    assert persisted_task_results(instance) == 0


def test_hybrid_query_preserves_context_selection_and_feedback(tmp_path: Path) -> None:
    """A real Wave query consumes immutable context settings and retains its exact source."""
    from nous.cognition import CognitiveQuery
    from nous.content.contracts import ContentSelection, SourceSelection
    from nous.runtime.bootstrap import open_instance
    from nous.temporal.values import Instant, Interval, Known, TimePoint, parse_explicit_time
    from nous.wave.contracts import (
        CandidateFilter,
        ContextContent,
        ContextFragment,
        TimeCondition,
        WaveQuery,
    )

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            request = TypeAdapter(dict[str, JsonValue]).validate_json(
                self.rfile.read(int(self.headers["Content-Length"]))
            )
            inputs = request["input"]
            count = len(inputs) if isinstance(inputs, list) else 1
            body = json.dumps(
                {
                    "object": "list",
                    "model": "local-embedding",
                    "data": [
                        {"object": "embedding", "index": index, "embedding": [0.5, 0.25]}
                        for index in range(count)
                    ],
                    "usage": {"prompt_tokens": count, "total_tokens": count},
                }
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format: str, *args: object) -> None:
            pass

    instance = tmp_path / "context-feedback"
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def scenario() -> None:
            async with open_configuration_operations(instance, environment={}) as configuration:
                await configuration.update({"cognition.search.space": "test-space"})
            async with open_instance(instance, environment={}) as runtime:
                cognition = await runtime.cognition()
                note = await cognition.records.add_note(
                    "Moonlight reflects borrowed light.", remember=True
                )
                view = await cognition.records.read(note.record.revision_ref)
                selector = SourceSelection(start_byte=0, end_byte=9)
                selected = ContentSelection(artifact=view.body_artifact, selection=selector)
                request = CognitiveQuery(
                    text="How can help become a source of light?",
                    mode="hybrid",
                    context=(
                        ContextFragment(
                            key="source-excerpt",
                            role="user",
                            sequence=1,
                            content=ContextContent(
                                artifact=view.body_artifact.identifier, selector=selector
                            ),
                        ),
                    ),
                )
                answer = await cognition.retrieval.query(request)
                assert answer.groups and answer.context.selected_context[0].content == selected
                recorded = view.times.recorded
                assert recorded is not None
                window = Interval(
                    start=Known(at=recorded),
                    end=Known(at=recorded.model_copy(update={"ticks": recorded.ticks + 1})),
                    meaning="duration",
                )
                filtered = await cognition.retrieval.query(
                    CognitiveQuery(
                        text="Moonlight",
                        mode="hybrid",
                        filters=CandidateFilter(
                            times=(TimeCondition(axis="recorded", range=window),)
                        ),
                    )
                )
                assert [group.revision_ref for group in filtered.groups] == [
                    note.record.revision_ref
                ]
                # Manual collections cannot replace the artifact's actual recorded time.
                manifest = tmp_path / "manual-time.json"
                manifest.write_text(
                    json.dumps(
                        {
                            "name": "manual-recorded-time",
                            "documents": [
                                {
                                    "key": "body",
                                    "input": {"artifact": view.body_artifact.identifier},
                                    "times": {"recorded": {"domain_id": "utc", "ticks": 0}},
                                }
                            ],
                            "candidates": [
                                {"key": "body", "document": "body", "content": {"document": "body"}}
                            ],
                        }
                    ),
                    encoding="utf-8",
                )
                snapshot = cast(dict[str, str], (await runtime.wave.build(manifest))["snapshot"])[
                    "identifier"
                ]
                epoch = TimePoint(domain_id="utc", ticks=0)
                spoofed = Interval(
                    start=Known(at=epoch),
                    end=Known(at=epoch.model_copy(update={"ticks": 1})),
                    meaning="duration",
                )

                def temporal_filter(value: Interval) -> CandidateFilter:
                    return CandidateFilter(times=(TimeCondition(axis="recorded", range=value),))

                old = await runtime.wave.query(
                    WaveQuery(
                        snapshot=snapshot,
                        text="Moonlight",
                        mode="lexical",
                        candidate_filter=temporal_filter(spoofed),
                    )
                )
                assert not old["hits"]
                artifact = await runtime.artifacts.get(view.body_artifact)
                actual = parse_explicit_time(artifact.recorded_at.isoformat())
                assert isinstance(actual, Instant)
                actual_window = Interval(
                    start=Known(at=actual.at),
                    end=Known(at=actual.at.model_copy(update={"ticks": actual.at.ticks + 1})),
                    meaning="duration",
                )
                current = await runtime.wave.query(
                    WaveQuery(
                        snapshot=snapshot,
                        text="Moonlight",
                        mode="lexical",
                        candidate_filter=temporal_filter(actual_window),
                    )
                )
                assert len(cast(list[object], current["hits"])) == 1
                feedback = await cognition.feedback.record_query(
                    answer, "retrieval", target=answer.delivery[0]
                )
            async with open_instance(instance, environment={}) as reopened:
                stored = await (await reopened.cognition()).feedback.read(feedback.identifier)
                assert stored.target.query is not None
                assert (
                    stored.target.query.context.selected_context == answer.context.selected_context
                )

        asyncio.run(scenario())
