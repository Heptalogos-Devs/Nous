"""The local runtime's user-visible path and its persistent contracts."""

import asyncio
import hashlib
import io
import json
import os
import subprocess
import sys
import threading
from fractions import Fraction
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import numpy as np
import pytest

from nous.application.configuration import open_configuration_operations
from nous.common.errors import NousError
from nous.content.contracts import ContentSelection, InputGroup, InputPart, OutputContent
from nous.models.embeddings import resolved_space
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.runtime.instance import InstanceLock
from nous.temporal.domains import TimeDomainDefinition, register_domain
from nous.temporal.values import (
    Instant,
    Interval,
    Known,
    TimePoint,
    Unbounded,
    UnknownBoundary,
    compare_points,
    match_range,
    parse_explicit_time,
)


def test_local_cli_initialization_configuration_and_runtime(tmp_path: Path) -> None:
    instance = tmp_path / "独立 instance"
    cwd = tmp_path / "another working directory"
    cwd.mkdir()
    environment = {key: value for key, value in os.environ.items() if not key.startswith("NOUS__")}
    command = [sys.executable, "-c", "from nous.transports.cli import main; main()"]

    def invoke(*args: str, env: dict[str, str] | None = None, success: bool = True):
        result = subprocess.run(
            [*command, "--instance", str(instance), "--json", *args],
            cwd=cwd,
            env=env or environment,
            capture_output=True,
            encoding="utf-8",
            timeout=45,
            check=False,
        )
        assert (result.returncode == 0) == success, (result.stdout, result.stderr)
        return json.loads(result.stdout)

    initialized = invoke("init", "--layout", "portable")
    identity = initialized["instance"]["instance_id"]
    assert (instance / "nous.toml").is_file()
    assert (instance / "data" / "nous.sqlite3").is_file()
    assert invoke("init")["instance"]["instance_id"] == identity

    fields = invoke("config", "list", "--level", "advanced")["fields"]
    assert {"logging.level", "execution.max_in_flight"} <= {field["path"] for field in fields}
    assert invoke("config", "set", "logging.level", "DEBUG")["state"] == "saved"
    invoke("config", "set", "execution.max_in_flight", "7")
    invoke("config", "set", "paths.cache_dir", "relative-cache")
    override = {**environment, "NOUS__LOGGING__LEVEL": "WARNING"}
    explained = invoke("config", "explain", "logging.level", env=override)
    assert explained["saved_value"] == "DEBUG"
    assert explained["effective_value"] == "WARNING"
    assert explained["source"]["layer"] == "environment"

    invoke("config", "set", "network.proxy_url", "--null")
    assert invoke("config", "explain", "network.proxy_url")["source"]["layer"] == "instance"
    invoke("config", "unset", "network.proxy_url")
    assert invoke("config", "explain", "network.proxy_url")["source"]["layer"] == "default"

    status = invoke("runtime", "check")
    assert status["state"] == "stopped"
    assert status["database"]["instance_id"] == identity
    assert status["database"]["foreign_keys"] is True
    assert status["bindings"]["time"] == "checked"
    assert status["bindings"]["embedding"] == "unconfigured"
    assert invoke("config", "show", "--effective")["values"]["execution"]["max_in_flight"] == 7
    parsed = invoke("time", "parse", "2026-11-03", "--timezone", "Asia/Shanghai")["value"]
    assert parsed["meaning"] == "uncertain_instant" and parsed["precision"] == "day"
    logs = (instance / "logs" / "nous.jsonl").read_text(encoding="utf-8").splitlines()
    assert any(json.loads(line).get("trace_id") for line in logs)

    with InstanceLock(instance):
        locked = invoke("config", "set", "logging.level", "INFO", success=False)
    assert locked["error"]["code"] == "instance.in_use"
    moved = invoke("config", "set", "paths.data_dir", "elsewhere", success=False)
    assert moved["error"]["code"] == "configuration.relocation_required"
    error_en = invoke("--language", "en", "config", "explain", "missing.path", success=False)
    error_zh = invoke("--language", "zh-CN", "config", "explain", "missing.path", success=False)
    assert error_en["error"]["code"] == error_zh["error"]["code"]
    assert error_en["error"]["message"] != error_zh["error"]["message"]

    moved_instance = tmp_path / "移动后的 portable instance"
    instance.rename(moved_instance)
    instance = moved_instance
    shown = invoke("instance", "show")
    assert Path(shown["paths"]["cache"]) == moved_instance / "relative-cache"
    assert invoke("runtime", "check")["database"]["instance_id"] == identity


def test_runtime_snapshots_transactions_and_time_domain_bindings(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance)
        async with open_instance(instance) as runtime:
            old = runtime.configuration.snapshot
            result = await runtime.configuration.update({"execution.default_timeout_seconds": 2})
            assert result["state"] == "applied"
            assert old.typed_values().execution.default_timeout_seconds is None
            assert runtime.configuration.snapshot.typed_values().execution.default_timeout_seconds == 2

            mars = {"kind": "linear", "name": "Mars", "tick_seconds": "1/1000", "epoch": "sol 0"}
            assert (await runtime.configuration.update({"time.domains.mars": mars}))[
                "state"
            ] == "applied"
            assert (await runtime.time.domains())["mars"].tick_seconds == Fraction(1, 1000)
            with pytest.raises(NousError) as failure:
                await runtime.configuration.update({"time.domains.mars.tick_seconds": "1/2"})
            assert failure.value.code == "configuration.binding_conflict"

            with pytest.raises(RuntimeError):
                async with runtime.database.write() as tx:
                    await register_domain(
                        TimeDomainDefinition(domain_id="rolled-back", kind="opaque", name="temp"),
                        tx,
                    )
                    raise RuntimeError("abort the business operation")
            assert "rolled-back" not in await runtime.time.domains()

            before_restart = runtime.configuration.snapshot
            result = await runtime.configuration.update(
                {"execution.max_in_flight": 3, "logging.level": "ERROR"}
            )
            assert result["state"] == "saved_pending_restart"
            assert runtime.configuration.snapshot is before_restart
        async with open_instance(instance) as reopened:
            assert reopened.configuration.snapshot.typed_values().execution.max_in_flight == 3
            assert reopened.configuration.snapshot.typed_values().logging.level == "ERROR"
            assert (await reopened.time.domains())["mars"].tick_seconds == Fraction(1, 1000)
            result = await reopened.configuration.update(
                {
                    "providers.gateway": {
                        "protocol": "openai-chat-completions",
                        "endpoint": "http://127.0.0.1:3000/v1",
                    },
                    "models.embedder": {
                        "provider": "gateway",
                        "model_name": "example-embedding",
                        "capabilities": ["embedding"],
                    },
                    "embedding.spaces.example": {
                        "model": "embedder",
                        "deployment_revision": "one",
                        "dimensions": 384,
                        "modalities": ["text"],
                        "normalization": "l2",
                        "encoding": {
                            "document_instruction": "",
                            "query_instruction": "",
                            "preprocessing_revision": "one",
                            "input_combination": "text",
                        },
                    },
                }
            )
            assert result["state"] == "saved_pending_restart"
        async with open_instance(instance) as reopened:
            result = await reopened.configuration.update(
                {"embedding.spaces.example.dimensions": 256}
            )
            assert result["state"] == "applied"
            definition = resolved_space(reopened.configuration.snapshot, "example")
            source = await reopened.artifacts.ingest("A document for the embedding space.")
            buffer = io.BytesIO()
            np.save(buffer, np.full((1, 256), 1 / 16, dtype=np.float32), allow_pickle=False)
            prepared = await reopened.results.prepare_outputs(
                {
                    "vectors": OutputContent(
                        data=buffer.getvalue(),
                        media_type="application/x-npy",
                        display_name="vectors.npy",
                    )
                }
            )
            async with reopened.database.write() as tx:
                await reopened.embedding_spaces.bind_in(tx, definition)
                adopted = await reopened.results.adopt_in(
                    tx,
                    operation="embedding",
                    producer={"model": "example-embedding"},
                    inputs=(
                        InputGroup(
                            key="S1",
                            slot_key="S1",
                            name="source",
                            parts=(InputPart(content=ContentSelection(artifact=source.ref)),),
                        ),
                    ),
                    prepared_outputs=prepared,
                )
                await reopened.embedding_spaces.attach_in(
                    tx,
                    definition=definition,
                    result_id=adopted.ref.id,
                    artifact_id=adopted.outputs[0].artifact.id,
                    purpose="document",
                    input_digests=[
                        hashlib.sha256(b"A document for the embedding space.").hexdigest()
                    ],
                )
            for patch in (
                {"embedding.spaces.example.dimensions": 128},
                {"models.embedder.model_name": "another-deployment"},
            ):
                with pytest.raises(NousError) as conflict:
                    await reopened.configuration.update(patch)
                assert conflict.value.code == "configuration.binding_conflict"
            assert (await reopened.results.get(adopted.ref)).inputs[0].parts[
                0
            ].content.artifact == source.ref

    asyncio.run(scenario())


def test_model_cli_fallback_and_structured_output(tmp_path: Path) -> None:
    received_headers: list[dict[str, str]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            received_headers.append(dict(self.headers))
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            content = '["red", "blue"]' if "response_format" in request else "A saved answer."
            body = json.dumps(
                {
                    "id": "response-1",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "test-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": content},
                            "finish_reason": "stop",
                        }
                    ],
                    "usage": {"prompt_tokens": 12, "completion_tokens": 4, "total_tokens": 16},
                }
            ).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format: str, *args: object) -> None:
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    instance = tmp_path / "fallback"

    async def setup() -> None:
        await initialize_instance(instance, environment={})
        async with open_configuration_operations(instance, environment={}) as config:
            await config.update(
                {
                    "providers.dead": {
                        "protocol": "openai-chat-completions",
                        "endpoint": "http://127.0.0.1:9/v1",
                        "network": {"connect_timeout_seconds": 1},
                    },
                    "providers.local": {
                        "protocol": "openai-chat-completions",
                        "endpoint": f"http://127.0.0.1:{server.server_port}/v1",
                    },
                    "models.first": {
                        "provider": "dead",
                        "model_name": "test-model",
                        "capabilities": ["text"],
                    },
                    "models.second": {
                        "provider": "local",
                        "model_name": "test-model",
                        "capabilities": ["text", "structured_output"],
                        "output_modes": ["text", "native"],
                    },
                    "tasks.summary": {
                        "model": "first",
                        "prompt": "summarize",
                        "fallback": [{"model": "second"}],
                    },
                    "tasks.custom": {
                        "model": "second",
                        "prompt": "custom-reading",
                        "output_mode": "native",
                    },
                }
            )

    try:
        asyncio.run(setup())
        command = [
            sys.executable,
            "-c",
            "from nous.transports.cli import main; main()",
            "--instance",
            str(instance),
            "--json",
        ]
        generated = subprocess.run(
            [
                *command,
                "tools",
                "model",
                "run",
                "--task",
                "summary",
                "--text",
                "Source text.",
                "--show-request",
            ],
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=45,
            check=True,
        )
        result = json.loads(generated.stdout)["results"][0]
        assert result["parameters"]["attempts"] == 2
        assert '"prompt_id": "summarize"' in generated.stderr
        assert result["producer"]["model_id"] == "second"
        assert result["outputs"][0]["view"]["body_text"] == "A saved answer."
        assert len(received_headers) == 1
        assert not any(name.lower() == "authorization" for name in received_headers[0])
        reopened = subprocess.run(
            [*command, "tools", "result", "read", result["ref"]["address"], "--sources"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=45,
            check=True,
        )
        restored = json.loads(reopened.stdout)
        assert restored["outputs"][0]["view"]["body_text"] == "A saved answer."
        assert restored["inputs"][0]["parts"][0]["content"]["artifact"]["address"].startswith(
            "art:"
        )

        schema = tmp_path / "array-schema.json"
        schema.write_text(
            json.dumps({"type": "array", "items": {"type": "string"}}), encoding="utf-8"
        )
        structured = subprocess.run(
            [
                *command,
                "tools",
                "model",
                "run",
                "--task",
                "custom",
                "--schema",
                str(schema),
                "--text",
                "Two colors.",
            ],
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=45,
            check=True,
        )
        output = json.loads(structured.stdout)["results"][0]["outputs"][0]
        assert output["view"]["media_type"] == "application/json"
        assert json.loads(output["view"]["body_text"]) == ["red", "blue"]
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def test_temporal_precision_unknown_boundaries_and_dst() -> None:
    day = parse_explicit_time("2026-11-03", timezone="Asia/Shanghai")
    assert isinstance(day, Interval) and isinstance(day.start, Known)
    beginning = day.start.at.ticks
    hour = Interval(
        start=Known(at=TimePoint(domain_id="utc", ticks=beginning)),
        end=Known(at=TimePoint(domain_id="utc", ticks=beginning + 3_600_000_000)),
        meaning="duration",
    )
    assert match_range(day, hour) == "unknown"
    assert match_range(day, day) == "matches"
    open_query = Interval(start=Unbounded(), end=Unbounded(), meaning="duration")
    uncertain = Interval(start=UnknownBoundary(), end=UnknownBoundary(), meaning="duration")
    assert match_range(uncertain, open_query) == "matches"
    assert match_range(Instant(at=TimePoint(domain_id="utc", ticks=beginning)), hour) == "matches"
    with pytest.raises(NousError) as mismatch:
        compare_points(TimePoint(domain_id="mars", ticks=0), TimePoint(domain_id="utc", ticks=0))
    assert mismatch.value.code == "time.domain_mismatch"
    with pytest.raises(NousError) as nonexistent:
        parse_explicit_time("2026-03-08T02:30:00", timezone="America/New_York")
    assert nonexistent.value.code == "time.nonexistent_local_time"
    with pytest.raises(NousError) as ambiguous:
        parse_explicit_time("2026-11-01T01:30:00", timezone="America/New_York")
    assert ambiguous.value.code == "time.ambiguous_local_time"
    early = parse_explicit_time("2026-11-01T01:30:00", timezone="America/New_York", fold=0)
    late = parse_explicit_time("2026-11-01T01:30:00", timezone="America/New_York", fold=1)
    assert isinstance(early, Instant) and isinstance(late, Instant)
    assert late.at.ticks - early.at.ticks == 3_600_000_000


def test_shutdown_waits_for_cancelled_blocking_worker(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance)
        ready = asyncio.get_running_loop().create_future()
        stop = asyncio.Event()
        started = threading.Event()
        release = threading.Event()

        async def host() -> None:
            async with open_instance(instance) as runtime:
                ready.set_result(runtime)
                await stop.wait()

        def work() -> None:
            started.set()
            if not release.wait(5):
                raise TimeoutError("worker was not released")

        host_task = asyncio.create_task(host())
        runtime = await ready
        waiter = asyncio.create_task(runtime.execution.blocking(work))
        try:
            assert await asyncio.to_thread(started.wait, 2)
            waiter.cancel()
            stop.set()
            await asyncio.sleep(0.05)
            assert not waiter.done()
            assert not host_task.done()
            assert (await runtime.database.check()).foreign_keys
        finally:
            release.set()
            stop.set()
            with pytest.raises(asyncio.CancelledError):
                await asyncio.wait_for(waiter, 5)
            await asyncio.wait_for(host_task, 5)
        assert runtime.state == "stopped"

    asyncio.run(scenario())


def test_cli_invalid_arguments_and_media_fail_as_json(tmp_path: Path) -> None:
    instance = tmp_path / "invalid-cli"
    asyncio.run(initialize_instance(instance, environment={}))
    invalid_pdf = tmp_path / "broken.pdf"
    invalid_pdf.write_bytes(b"%PDF-1.7\nnot a readable document")
    text = tmp_path / "source.txt"
    text.write_text("A source", encoding="utf-8")
    command = [
        sys.executable,
        "-c",
        "from nous.transports.cli import main; main()",
        "--instance",
        str(instance),
        "--json",
    ]
    for arguments, code in (
        (("tools", "media", "analyze"), "input.invalid"),
        (("tools", "media", "analyze", str(text), "--strategy", "wrong"), "input.invalid"),
        (("not-a-command",), "input.invalid"),
        (("tools", "media", "inspect", str(invalid_pdf)), "media.invalid_pdf"),
        (
            (
                "tools",
                "media",
                "analyze",
                str(text),
                "--strategy",
                "description_only",
                "--from",
                "inf",
            ),
            "media.invalid_selection",
        ),
        (
            (
                "tools",
                "media",
                "analyze",
                str(text),
                "--strategy",
                "description_only",
                "--to",
                "nan",
            ),
            "media.invalid_selection",
        ),
    ):
        result = subprocess.run(
            [*command, *arguments], capture_output=True, text=True, encoding="utf-8", timeout=30
        )
        assert result.returncode != 0
        assert json.loads(result.stdout)["error"]["code"] == code
        assert "Traceback" not in result.stderr
