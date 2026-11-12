"""Local clients retain their streams, files and progress while a host is running."""

import asyncio
import json
import os
import select
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler
from pathlib import Path

import pytest

from nous.application.configuration import open_configuration_operations
from nous.runtime.instance import InstanceLocator
from nous.transports.cli.control_wire import endpoint, read_endpoint
from tests.cli.shared.host import command, initialize, wait_for
from tests.integration.shared.models import configure_models, model_server


def _start(instance: Path, cwd: Path) -> subprocess.Popen[bytes]:
    return subprocess.Popen(
        [
            sys.executable,
            "-c",
            "from nous.transports.cli import main; main()",
            "--instance",
            str(instance),
            "runtime",
            "run",
        ],
        cwd=cwd,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def _stop(host: subprocess.Popen[bytes]) -> None:
    if host.poll() is None:
        if os.name == "nt":
            host.terminate()
        else:
            host.send_signal(signal.SIGINT)
        host.wait(timeout=30)


def test_embedding_probe_reads_selected_client_range_without_accepting_file(tmp_path: Path) -> None:
    from nous.application.embedding import (
        PROBE_EMBEDDING,
        RUN_EMBEDDING,
        ProbeEmbedding,
        RunEmbedding,
    )
    from nous.application.inputs import FileInput
    from nous.content.contracts import ExternalInput, SourceSelection
    from nous.transports.cli.local_control import request_call

    submissions: list[dict[str, object]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            submissions.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            data = json.dumps(
                {
                    "object": "list",
                    "model": "local-embedding",
                    "data": [{"object": "embedding", "index": 0, "embedding": [0.5, 0.25]}],
                }
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, format: str, *args: object) -> None:
            pass

    instance = tmp_path / "instance"
    source = tmp_path / "large.txt"
    prefix = b"x" * 100_000
    source.write_bytes(prefix + b"\r\nkept\r\nignored")
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def configure() -> None:
            async with open_configuration_operations(instance, environment={}) as config:
                await config.update({"content.text_read_max_bytes": 16})

        asyncio.run(configure())
        path = endpoint(InstanceLocator(instance))
        assert path is not None
        host = _start(instance, tmp_path)
        try:
            wait_for(lambda: path.exists() or host.poll() is not None)
            selected = ExternalInput(
                key="source", selection=SourceSelection(start_line=2, end_line=2)
            )
            result = request_call(
                InstanceLocator(instance),
                PROBE_EMBEDDING.name,
                ProbeEmbedding(model="vectors", sources=(selected,)),
                inputs={"source": FileInput(source)},
            )
            assert result is not None and "error" not in result
            assert submissions[-1]["input"] == ["kept\r\n"]
            with sqlite3.connect(instance / "data" / "nous.sqlite3") as database:
                assert database.execute("SELECT count(*) FROM content_artifacts").fetchone()[0] == 0
            result = request_call(
                InstanceLocator(instance),
                RUN_EMBEDDING.name,
                RunEmbedding(space="test-space", purpose="document", sources=(selected,)),
                inputs={"source": FileInput(source)},
            )
            assert result is not None and "error" not in result
            assert submissions[-1]["input"] == ["kept\r\n"]
        finally:
            _stop(host)


def test_idle_unauthenticated_connections_do_not_starve_cli(tmp_path: Path) -> None:
    instance = tmp_path / "instance"
    asyncio.run(initialize(instance))
    path = endpoint(InstanceLocator(instance))
    assert path is not None
    host = _start(instance, tmp_path)
    peers: list[socket.socket] = []
    try:
        wait_for(lambda: path.exists() or host.poll() is not None)
        info = read_endpoint(path)
        assert info is not None
        peers = [
            socket.create_connection(("127.0.0.1", info["port"]), timeout=3) for _ in range(32)
        ]
        start = time.monotonic()
        result = command(instance, "journal", "list")
        assert result.returncode == 0, result.stderr
        assert time.monotonic() - start < 8
        program = (
            "import nous.transports.cli.local_control as c; "
            "import nous.transports.cli.control_wire as w; "
            "c.MAX_FRAME=w.MAX_FRAME=1024; "
            "from nous.transports.cli import main; main()"
        )
        oversized = subprocess.run(
            [
                sys.executable,
                "-c",
                program,
                "--instance",
                str(instance),
                "--json",
                "note",
                "add",
                "--text",
                "a" * 5000,
            ],
            text=True,
            capture_output=True,
            timeout=15,
            check=False,
        )
        assert oversized.returncode != 0
        assert "limit" in oversized.stdout.lower()
    finally:
        for peer in peers:
            peer.close()
        _stop(host)


def test_open_stdin_obeys_deadline_without_blocking_host(tmp_path: Path) -> None:
    instance = tmp_path / "instance"
    asyncio.run(initialize(instance))

    async def configure() -> None:
        async with open_configuration_operations(instance, environment={}) as settings:
            await settings.update({"execution.default_timeout_seconds": 1.5})

    asyncio.run(configure())
    endpoint_path = endpoint(InstanceLocator(instance))
    assert endpoint_path is not None
    host = _start(instance, tmp_path)
    client: subprocess.Popen[bytes] | None = None
    try:
        wait_for(lambda: endpoint_path.exists() or host.poll() is not None)
        client = subprocess.Popen(
            [
                sys.executable,
                "-c",
                "from nous.transports.cli import main; main()",
                "--instance",
                str(instance),
                "--json",
                "note",
                "add",
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        assert client.wait(timeout=15) != 0
        assert client.stdout is not None
        assert b"operation.timeout" in client.stdout.read()
        if client.stdin is not None:
            client.stdin.close()
        wait_for(lambda: not list(endpoint_path.parent.glob(f".{endpoint_path.stem}.input-*")))
        assert command(instance, "journal", "list").returncode == 0
    finally:
        if client is not None and client.poll() is None:
            client.kill()
            client.wait(timeout=10)
        _stop(host)


def test_wave_manifest_binds_paths_relative_to_client(tmp_path: Path) -> None:
    instance, client_dir, host_dir = tmp_path / "instance", tmp_path / "client", tmp_path / "host"
    client_dir.mkdir()
    host_dir.mkdir()
    asyncio.run(initialize(instance))
    (client_dir / "document.md").write_text("CLIENT_CONTENT_MARKER", encoding="utf-8")
    (host_dir / "document.md").write_text("HOST_CONTENT_MARKER", encoding="utf-8")
    (client_dir / "manifest.json").write_text(
        json.dumps(
            {
                "name": "client-corpus",
                "documents": [{"key": "doc", "input": {"file": "document.md"}}],
                "candidates": [{"key": "doc", "document": "doc", "content": {"document": "doc"}}],
            }
        ),
        encoding="utf-8",
    )
    endpoint_path = endpoint(InstanceLocator(instance))
    assert endpoint_path is not None
    host = _start(instance, host_dir)
    try:
        wait_for(lambda: endpoint_path.exists() or host.poll() is not None)
        built = command(
            instance, "tools", "wave", "build", "--manifest", "manifest.json", cwd=client_dir
        )
        assert built.returncode == 0, (built.stdout, built.stderr)
        snapshot = json.loads(built.stdout)["snapshot"]["identifier"]
        result = command(
            instance,
            "tools",
            "wave",
            "query",
            snapshot,
            "CLIENT_CONTENT_MARKER",
            "--mode",
            "lexical",
        )
        assert result.returncode == 0, (result.stdout, result.stderr)
        assert "CLIENT_CONTENT_MARKER" in result.stdout
        assert "HOST_CONTENT_MARKER" not in result.stdout
    finally:
        _stop(host)


@pytest.mark.skipif(os.name == "nt", reason="POSIX FIFO and select are required")
def test_fifo_timeout_does_not_block_host_or_shutdown(tmp_path: Path) -> None:
    instance = tmp_path / "instance"
    asyncio.run(initialize(instance))

    async def configure() -> None:
        async with open_configuration_operations(instance, environment={}) as settings:
            await settings.update({"execution.default_timeout_seconds": 2})

    asyncio.run(configure())
    fifo = tmp_path / "source.fifo"
    os.mkfifo(fifo)
    path = endpoint(InstanceLocator(instance))
    assert path is not None
    host = _start(instance, tmp_path)
    client = None
    probe = None
    try:
        wait_for(lambda: path.exists() or host.poll() is not None)
        conflict = command(
            instance, "note", "add", "--text", "already supplied", "--file", str(fifo), data=""
        )
        assert conflict.returncode != 0 and "input.conflicting_body" in conflict.stdout
        missing = command(
            instance, "tools", "wave", "build", "--manifest", "missing.json", cwd=tmp_path
        )
        assert missing.returncode != 0 and "input.unreadable" in missing.stdout
        # Other file-consuming command families use the same client boundary.
        for arguments in (
            ("query", "anything", "--context", str(fifo)),
            ("config", "set", "logging.level", "--value-file", str(fifo)),
            ("note", "patch", "note(nonexistent)", "--changes", str(fifo)),
            ("tools", "wave", "build", "--manifest", str(fifo)),
        ):
            blocked = command(instance, *arguments, data="")
            assert blocked.returncode != 0 and "operation.timeout" in blocked.stdout, blocked.stdout
            assert command(instance, "journal", "list").returncode == 0
        # Load the real CLI and its request dependencies before measuring;
        # release it only after the other request has reached the FIFO.
        probe_program = """import sys
from nous.transports.cli import main
import nous.application.dispatch
import nous.transports.cli.io
import nous.transports.cli.local_control
print("ready", flush=True)
sys.stdin.buffer.read(1)
main()
"""
        probe = subprocess.Popen(
            [
                sys.executable,
                "-c",
                probe_program,
                "--instance",
                str(instance),
                "--json",
                "journal",
                "list",
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        assert probe.stdout is not None
        ready, _, _ = select.select([probe.stdout], [], [], 30)
        assert ready and probe.stdout.readline() == b"ready\n"
        wait_for(lambda: not list(path.parent.glob(f".{path.stem}.input-*")), timeout=10)
        client = subprocess.Popen(
            [
                sys.executable,
                "-c",
                "from nous.transports.cli import main; main()",
                "--instance",
                str(instance),
                "--json",
                "note",
                "add",
                "--file",
                str(fifo),
            ],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        wait_for(lambda: bool(list(path.parent.glob(f".{path.stem}.input-*"))), timeout=10)
        assert client.poll() is None
        started = time.monotonic()
        check_out, check_err = probe.communicate(input=b"x", timeout=10)
        assert probe.returncode == 0, (check_out, check_err)
        assert time.monotonic() - started < 5
        assert client.poll() is None
        assert list(path.parent.glob(f".{path.stem}.input-*"))
        out, err = client.communicate(timeout=12)
        assert client.returncode != 0, (out, err)
        assert b"operation.timeout" in out + err
        client = subprocess.Popen(
            [
                sys.executable,
                "-c",
                "from nous.transports.cli import main; main()",
                "--instance",
                str(instance),
                "--json",
                "note",
                "add",
                "--file",
                str(fifo),
            ],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        time.sleep(0.4)
        client.kill()
        client.wait(timeout=5)
        _stop(host)
        assert not path.exists()
        host = _start(instance, tmp_path)
        wait_for(lambda: path.exists() or host.poll() is not None)
        previous = read_endpoint(path)
        assert previous is not None
        client = subprocess.Popen(
            [
                sys.executable,
                "-c",
                "from nous.transports.cli import main; main()",
                "--instance",
                str(instance),
                "--json",
                "note",
                "add",
                "--file",
                str(fifo),
            ],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        wait_for(lambda: bool(list(path.parent.glob(f".{path.stem}.input-*"))), timeout=10)
        host.kill()
        host.wait(timeout=10)
        client.wait(timeout=10)
        host = _start(instance, tmp_path)

        def restarted() -> bool:
            current = read_endpoint(path)
            return current is not None and current["secret"] != previous["secret"]

        wait_for(restarted)
        assert not list(path.parent.glob(f".{path.stem}.input-*"))
        assert command(instance, "journal", "list").returncode == 0
    finally:
        if probe is not None and probe.poll() is None:
            probe.kill()
            probe.wait(timeout=10)
        if client is not None and client.poll() is None:
            client.kill()
            client.wait()
        _stop(host)


@pytest.mark.skipif(os.name == "nt", reason="pipe readiness test requires select on POSIX pipes")
def test_relative_inputs_draft_stdin_and_live_model_output(tmp_path: Path) -> None:
    arrived, permit_delta, sent, release = (threading.Event() for _ in range(4))
    embedding_inputs: list[object] = []
    model_requests: list[dict[str, object]] = []

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_POST(self) -> None:
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            base = {"id": "host-test", "model": "local-model", "created": 1700000000}
            if self.path.endswith("/embeddings"):
                embedding_inputs.append(request.get("input"))
                payload = json.dumps(
                    {
                        "object": "list",
                        "model": "local-embedding",
                        "data": [{"object": "embedding", "index": 0, "embedding": [0.6, 0.8]}],
                        "usage": {"prompt_tokens": 1, "total_tokens": 1},
                    }
                ).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
            elif request.get("stream"):
                model_requests.append(request)
                arrived.set()
                permit_delta.wait(30)
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Connection", "close")
                self.end_headers()
                chunk: dict[str, object] = {
                    **base,
                    "object": "chat.completion.chunk",
                    "choices": [
                        {
                            "index": 0,
                            "delta": {"role": "assistant", "content": "FIRST_DELTA"},
                            "finish_reason": None,
                        }
                    ],
                }
                self.wfile.write(("data: " + json.dumps(chunk) + "\n\n").encode())
                self.wfile.flush()
                sent.set()
                release.wait(30)
                chunk["choices"] = [{"index": 0, "delta": {}, "finish_reason": "stop"}]
                self.wfile.write(("data: " + json.dumps(chunk) + "\n\ndata: [DONE]\n\n").encode())
                self.wfile.flush()
                self.close_connection = True
            else:
                payload = json.dumps(
                    {
                        **base,
                        "object": "chat.completion",
                        "choices": [
                            {
                                "index": 0,
                                "message": {"role": "assistant", "content": "MODEL_ORIGINAL_DRAFT"},
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

    instance, host_dir, client_dir = tmp_path / "instance", tmp_path / "host", tmp_path / "client"
    host_dir.mkdir()
    client_dir.mkdir()
    (host_dir / "same.md").write_text("HOST_PRIVATE_DOCUMENT", encoding="utf-8")
    (client_dir / "same.md").write_text("CLIENT_SELECTED_DOCUMENT", encoding="utf-8")
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def configure() -> None:
            async with open_configuration_operations(instance, environment={}) as settings:
                await settings.update(
                    {
                        "tasks.journal-draft": {
                            "model": "reader",
                            "prompt": "journal-draft",
                            "output_mode": "text",
                        }
                    }
                )

        asyncio.run(configure())
        path = endpoint(InstanceLocator(instance))
        assert path is not None
        host = _start(instance, host_dir)
        process = None
        try:
            wait_for(lambda: path.exists() or host.poll() is not None)
            preview = command(
                instance,
                "tools",
                "prompt",
                "preview",
                "summarize",
                "--input",
                "same.md",
                cwd=client_dir,
            )
            assert preview.returncode == 0, preview.stderr
            assert "CLIENT_SELECTED_DOCUMENT" in preview.stdout
            assert "HOST_PRIVATE_DOCUMENT" not in preview.stdout
            embedding = command(
                instance,
                "tools",
                "embedding",
                "probe",
                "vectors",
                "--input",
                "same.md",
                cwd=client_dir,
            )
            assert embedding.returncode == 0, (embedding.stdout, embedding.stderr)
            assert "CLIENT_SELECTED_DOCUMENT" in json.dumps(embedding_inputs)
            assert "HOST_PRIVATE_DOCUMENT" not in json.dumps(embedding_inputs)
            added = command(instance, "tools", "content", "add", "same.md", cwd=client_dir)
            assert added.returncode == 0, (added.stdout, added.stderr)
            artifact = json.loads(added.stdout)["ref"]["identifier"]
            exported = command(
                instance,
                "tools",
                "content",
                "export",
                artifact,
                "--output",
                "result.md",
                cwd=client_dir,
            )
            assert exported.returncode == 0, (exported.stdout, exported.stderr)
            assert (client_dir / "result.md").read_text() == "CLIENT_SELECTED_DOCUMENT"
            conflict = command(
                instance,
                "tools",
                "content",
                "export",
                artifact,
                "--output",
                "result.md",
                cwd=client_dir,
            )
            assert conflict.returncode == 0
            assert (client_dir / "result.md").read_text() == "CLIENT_SELECTED_DOCUMENT"
            schema = command(
                instance, "config", "schema", "--output", "schema.json", cwd=client_dir
            )
            assert schema.returncode == 0, (schema.stdout, schema.stderr)
            assert json.loads((client_dir / "schema.json").read_text())
            prompt = client_dir / "prompt"
            shutil.copytree(
                Path(__file__).resolve().parents[3] / "src/nous/models/resources/summarize", prompt
            )
            os.mkfifo(prompt / "unrelated.fifo")
            imported = command(
                instance, "tools", "prompt", "import", "summarize", str(prompt), cwd=client_dir
            )
            assert imported.returncode == 0, (imported.stdout, imported.stderr)
            actual = client_dir / "actual"
            actual.mkdir()
            (actual / "document.md").write_text("CLIENT_CORPUS_CONTENT", encoding="utf-8")
            (host_dir / "document.md").write_text("HOST_WRONG_CORPUS", encoding="utf-8")
            (actual / "manifest.json").write_text(
                json.dumps(
                    {
                        "name": "client-symlink-corpus",
                        "documents": [{"key": "doc", "input": {"file": "document.md"}}],
                        "candidates": [
                            {"key": "doc", "document": "doc", "content": {"document": "doc"}}
                        ],
                    }
                ),
                encoding="utf-8",
            )
            (client_dir / "manifest.json").symlink_to(actual / "manifest.json")
            built = command(
                instance, "tools", "wave", "build", "--manifest", "manifest.json", cwd=client_dir
            )
            assert built.returncode == 0, (built.stdout, built.stderr)
            snapshot = json.loads(built.stdout)["snapshot"]["identifier"]
            wave_result = command(
                instance,
                "tools",
                "wave",
                "query",
                snapshot,
                "CLIENT_CORPUS_CONTENT",
                "--mode",
                "lexical",
                cwd=client_dir,
            )
            assert wave_result.returncode == 0, (wave_result.stdout, wave_result.stderr)
            assert "CLIENT_CORPUS_CONTENT" in wave_result.stdout
            assert "HOST_WRONG_CORPUS" not in wave_result.stdout
            process = subprocess.Popen(
                [
                    sys.executable,
                    "-c",
                    "from nous.transports.cli import main; main()",
                    "--instance",
                    str(instance),
                    "--json",
                    "tools",
                    "model",
                    "run",
                    "--task",
                    "read",
                    "--input",
                    "same.md",
                    "--stream",
                    "--show-request",
                ],
                cwd=client_dir,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            assert arrived.wait(30)
            assert process.stdout is not None and process.stderr is not None
            assert "CLIENT_SELECTED_DOCUMENT" in json.dumps(model_requests[-1])
            assert "HOST_PRIVATE_DOCUMENT" not in json.dumps(model_requests[-1])
            permit_delta.set()
            assert sent.wait(30)
            ready, _, _ = select.select([process.stdout], [], [], 4)
            assert ready and process.poll() is None
            delta_output = os.read(process.stdout.fileno(), 65536)
            assert b"FIRST_DELTA" in delta_output
            release.set()
            out, err = process.communicate(timeout=30)
            assert process.returncode == 0, (out, err)
            # All body consumers fetch stdin on demand; an unrelated command
            # does not wait for a still-open pipe.
            idle = subprocess.Popen(
                [
                    sys.executable,
                    "-c",
                    "from nous.transports.cli import main; main()",
                    "--instance",
                    str(instance),
                    "--json",
                    "journal",
                    "list",
                ],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            assert idle.wait(timeout=15) == 0
            if idle.stdin is not None:
                idle.stdin.close()
            note = command(instance, "note", "add", "--remember", data="NOTE_FROM_STDIN")
            assert note.returncode == 0, note.stderr
            ref = json.loads(note.stdout)["commit"]["ref"]["identifier"]
            replaced_note = command(
                instance, "note", "replace", ref + "@1", data="NOTE_REPLACED_STDIN"
            )
            assert replaced_note.returncode == 0, (replaced_note.stdout, replaced_note.stderr)
            assert "NOTE_REPLACED_STDIN" in command(instance, "read", ref).stdout
            journal = command(instance, "journal", "create", data="JOURNAL_FROM_STDIN")
            assert journal.returncode == 0, (journal.stdout, journal.stderr)
            journal_ref = json.loads(journal.stdout)["commit"]["ref"]["identifier"]
            replaced_journal = command(
                instance, "journal", "replace", journal_ref + "@1", data="JOURNAL_REPLACED_STDIN"
            )
            assert replaced_journal.returncode == 0, (
                replaced_journal.stdout,
                replaced_journal.stderr,
            )
            assert "JOURNAL_REPLACED_STDIN" in command(instance, "read", journal_ref).stdout
            draft = command(instance, "journalize", "--source", ref, "--draft")
            assert draft.returncode == 0, draft.stderr
            draft_ref = json.loads(draft.stdout)["draft"]["identifier"]
            revised = command(
                instance, "journal", "draft", "revise", draft_ref, data="USER_REVISED_BODY"
            )
            assert revised.returncode == 0, (revised.stdout, revised.stderr)
            revised_ref = json.loads(revised.stdout)["ref"]["identifier"]
            adopted = command(instance, "journal", "draft", "adopt", revised_ref)
            assert adopted.returncode == 0, adopted.stderr
            record = json.loads(adopted.stdout)["commit"]["ref"]["identifier"]
            read = command(instance, "read", record)
            assert "USER_REVISED_BODY" in read.stdout
            assert "MODEL_ORIGINAL_DRAFT" not in read.stdout
        finally:
            permit_delta.set()
            release.set()
            if process is not None and process.poll() is None:
                process.kill()
                process.wait()
            _stop(host)


def test_interrupted_export_preserves_target_and_saved_artifact(tmp_path: Path) -> None:
    from nous.runtime.bootstrap import open_instance

    instance = tmp_path / "instance"

    async def make_artifact() -> str:
        await initialize(instance)
        async with open_instance(instance, environment={}) as runtime:
            return (await runtime.artifacts.ingest(b"payload")).ref.identifier

    artifact = asyncio.run(make_artifact())
    endpoint_path = endpoint(InstanceLocator(instance))
    assert endpoint_path is not None
    host = _start(instance, tmp_path)
    client = None
    marker, target = tmp_path / "entered", tmp_path / "exported"
    target.write_bytes(b"existing user content")
    program = """import os, time
from pathlib import Path
import nous.storage.file_output as delivery
original = delivery.deliver_file
def stalled(source, destination, *, digest, length, cancelled=None):
    Path(os.environ["NOUS_TEST_MARKER"]).write_text("started")
    while cancelled is not None and not cancelled():
        time.sleep(0.05)
    return original(source, destination, digest=digest, length=length, cancelled=cancelled)
delivery.deliver_file = stalled
from nous.transports.cli import main
main()
"""
    try:
        wait_for(lambda: endpoint_path.exists() or host.poll() is not None)
        previous = read_endpoint(endpoint_path)
        assert previous is not None
        client = subprocess.Popen(
            [
                sys.executable,
                "-c",
                program,
                "--instance",
                str(instance),
                "--json",
                "tools",
                "content",
                "export",
                artifact,
                "--output",
                str(target),
            ],
            env={**os.environ, "NOUS_TEST_MARKER": str(marker)},
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        wait_for(marker.exists)
        host.kill()
        host.wait(timeout=10)
        assert client.wait(timeout=10) != 0
        assert target.read_bytes() == b"existing user content"
        assert not list(tmp_path.glob(".exported.*"))
        host = _start(instance, tmp_path)

        def restarted() -> bool:
            current = read_endpoint(endpoint_path)
            return current is not None and current["secret"] != previous["secret"]

        wait_for(restarted)
        assert command(instance, "journal", "list").returncode == 0
        exported = command(
            instance, "tools", "content", "export", artifact, "--output", str(target)
        )
        assert exported.returncode == 0, (exported.stdout, exported.stderr)
        assert target.read_bytes() == b"payload"
        repeated = command(
            instance, "tools", "content", "export", artifact, "--output", str(target)
        )
        assert repeated.returncode == 0 and target.read_bytes() == b"payload"
    finally:
        if client is not None and client.poll() is None:
            client.kill()
            client.wait(timeout=10)
        _stop(host)
