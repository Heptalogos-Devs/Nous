"""Local clients retain their streams, files and progress while a host is running."""

import asyncio
import json
import os
import select
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler
from pathlib import Path

import pytest
from model_support import configure_models, model_server
from test_local_control import _command, _initialize, _wait  # pyright: ignore[reportPrivateUsage]

from nous.application.configuration import open_configuration_operations
from nous.runtime.instance import InstanceLocator
from nous.transports.cli.control_wire import endpoint, read_endpoint


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


def test_idle_unauthenticated_connections_do_not_starve_cli(tmp_path: Path) -> None:
    instance = tmp_path / "instance"
    asyncio.run(_initialize(instance))
    path = endpoint(InstanceLocator(instance))
    assert path is not None
    host = _start(instance, tmp_path)
    peers: list[socket.socket] = []
    try:
        _wait(lambda: path.exists() or host.poll() is not None)
        info = read_endpoint(path)
        assert info is not None
        peers = [
            socket.create_connection(("127.0.0.1", info["port"]), timeout=3) for _ in range(32)
        ]
        start = time.monotonic()
        result = _command(instance, "journal", "list")
        assert result.returncode == 0, result.stderr
        assert time.monotonic() - start < 8
        program = (
            "import nous.transports.cli.local_control as c; "
            "import nous.transports.cli.control_wire as w; "
            "c.MAX_FRAME=w.MAX_FRAME=1024; "
            "from nous.transports.cli import main; main()"
        )
        oversized = subprocess.run(
            [sys.executable, "-c", program, "--instance", str(instance), "--json", "note", "add"],
            input="\n" * 700,
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


@pytest.mark.skipif(os.name == "nt", reason="POSIX FIFO and select are required")
def test_fifo_timeout_does_not_block_host_or_shutdown(tmp_path: Path) -> None:
    instance = tmp_path / "instance"
    asyncio.run(_initialize(instance))

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
    try:
        _wait(lambda: path.exists() or host.poll() is not None)
        conflict = _command(
            instance, "note", "add", "--text", "already supplied", "--file", str(fifo), data=""
        )
        assert conflict.returncode != 0 and "input.conflicting_body" in conflict.stdout
        missing = _command(
            instance, "tools", "wave", "build", "--manifest", "missing.json", cwd=tmp_path
        )
        assert missing.returncode != 0 and "input.invalid" in missing.stdout
        # Other file-consuming command families use the same client boundary.
        for arguments in (
            ("query", "anything", "--context", str(fifo)),
            ("config", "set", "logging.level", "--value-file", str(fifo)),
            ("note", "patch", "note(nonexistent)", "--changes", str(fifo)),
            ("tools", "wave", "build", "--manifest", str(fifo)),
        ):
            blocked = _command(instance, *arguments, data="")
            assert blocked.returncode != 0 and "operation.timeout" in blocked.stdout, blocked.stdout
            assert _command(instance, "journal", "list").returncode == 0
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
        started = time.monotonic()
        check = _command(instance, "journal", "list")
        assert check.returncode == 0, check.stderr
        assert time.monotonic() - started < 5
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
                "config",
                "schema",
                "--output",
                str(fifo),
            ],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        time.sleep(0.4)
        check = _command(instance, "journal", "list")
        assert check.returncode == 0, check.stderr
        out, err = client.communicate(timeout=12)
        assert client.returncode != 0 and b"operation.timeout" in out + err
        assert fifo.exists() and not fifo.is_file()
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
        _wait(lambda: path.exists() or host.poll() is not None)
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
        _wait(lambda: bool(list(path.parent.glob(f".{path.stem}.input-*"))), timeout=10)
        host.kill()
        host.wait(timeout=10)
        client.wait(timeout=10)
        host = _start(instance, tmp_path)

        def restarted() -> bool:
            current = read_endpoint(path)
            return current is not None and current["secret"] != previous["secret"]

        _wait(restarted)
        assert not list(path.parent.glob(f".{path.stem}.input-*"))
        assert _command(instance, "journal", "list").returncode == 0
    finally:
        if client is not None and client.poll() is None:
            client.kill()
            client.wait()
        _stop(host)


@pytest.mark.skipif(os.name == "nt", reason="pipe readiness test requires select on POSIX pipes")
def test_relative_inputs_draft_stdin_and_live_model_output(tmp_path: Path) -> None:
    arrived, permit_delta, sent, release = (threading.Event() for _ in range(4))
    embedding_inputs: list[object] = []

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
            _wait(lambda: path.exists() or host.poll() is not None)
            preview = _command(
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
            embedding = _command(
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
            added = _command(instance, "tools", "content", "add", "same.md", cwd=client_dir)
            assert added.returncode == 0, (added.stdout, added.stderr)
            artifact = json.loads(added.stdout)["ref"]["identifier"]
            exported = _command(
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
            conflict = _command(
                instance,
                "tools",
                "content",
                "export",
                artifact,
                "--output",
                "result.md",
                cwd=client_dir,
            )
            assert conflict.returncode != 0
            assert (client_dir / "result.md").read_text() == "CLIENT_SELECTED_DOCUMENT"
            schema = _command(
                instance, "config", "schema", "--output", "schema.json", cwd=client_dir
            )
            assert schema.returncode == 0, (schema.stdout, schema.stderr)
            assert json.loads((client_dir / "schema.json").read_text())
            prompt = client_dir / "prompt"
            shutil.copytree(
                Path(__file__).resolve().parents[2] / "src/nous/models/resources/summarize", prompt
            )
            os.mkfifo(prompt / "unrelated.fifo")
            imported = _command(
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
            built = _command(
                instance, "tools", "wave", "build", "--manifest", "manifest.json", cwd=client_dir
            )
            assert built.returncode == 0, (built.stdout, built.stderr)
            snapshot = json.loads(built.stdout)["snapshot"]["identifier"]
            wave_result = _command(
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
            ready, _, _ = select.select([process.stderr], [], [], 4)
            assert ready and process.poll() is None
            request_output = os.read(process.stderr.fileno(), 65536)
            assert b"CLIENT_SELECTED_DOCUMENT" in request_output
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
            note = _command(instance, "note", "add", "--remember", data="NOTE_FROM_STDIN")
            assert note.returncode == 0, note.stderr
            ref = json.loads(note.stdout)["record"]["ref"]["identifier"]
            replaced_note = _command(
                instance, "note", "replace", ref + "@1", data="NOTE_REPLACED_STDIN"
            )
            assert replaced_note.returncode == 0, (replaced_note.stdout, replaced_note.stderr)
            assert "NOTE_REPLACED_STDIN" in _command(instance, "read", ref).stdout
            journal = _command(instance, "journal", "create", data="JOURNAL_FROM_STDIN")
            assert journal.returncode == 0, (journal.stdout, journal.stderr)
            journal_ref = json.loads(journal.stdout)["record"]["ref"]["identifier"]
            replaced_journal = _command(
                instance, "journal", "replace", journal_ref + "@1", data="JOURNAL_REPLACED_STDIN"
            )
            assert replaced_journal.returncode == 0, (
                replaced_journal.stdout,
                replaced_journal.stderr,
            )
            assert "JOURNAL_REPLACED_STDIN" in _command(instance, "read", journal_ref).stdout
            draft = _command(instance, "journalize", "--source", ref, "--draft")
            assert draft.returncode == 0, draft.stderr
            draft_ref = json.loads(draft.stdout)["draft_ref"]["identifier"]
            adopted = _command(
                instance, "journal", "draft", "adopt", draft_ref, data="USER_REVISED_BODY"
            )
            assert adopted.returncode == 0, adopted.stderr
            record = json.loads(adopted.stdout)["record"]["ref"]["identifier"]
            read = _command(instance, "read", record)
            assert "USER_REVISED_BODY" in read.stdout
            assert "MODEL_ORIGINAL_DRAFT" not in read.stdout
        finally:
            permit_delta.set()
            release.set()
            if process is not None and process.poll() is None:
                process.kill()
                process.wait()
            _stop(host)


def test_export_staging_is_recovered_after_both_processes_crash(tmp_path: Path) -> None:
    from nous.runtime.bootstrap import open_instance

    instance = tmp_path / "instance"

    async def initialize() -> str:
        await _initialize(instance)
        async with open_instance(instance, environment={}) as runtime:
            return (await runtime.artifacts.ingest(b"payload")).ref.identifier

    artifact = asyncio.run(initialize())
    endpoint_path = endpoint(InstanceLocator(instance))
    assert endpoint_path is not None
    host = _start(instance, tmp_path)
    client = None
    marker, target = tmp_path / "entered", tmp_path / "exported"
    program = """import os, threading
from pathlib import Path
from nous.transports.cli.control_files import ClientFiles
original = ClientFiles.perform
def stalled(self, message):
    if message.get("kind") == "export_prepare":
        Path(message["temporary"]).write_bytes(b"partial staged bytes")
        Path(os.environ["NOUS_TEST_MARKER"]).write_text("started")
        threading.Event().wait()
    return original(self, message)
ClientFiles.perform = stalled
from nous.transports.cli import main
main()
"""
    try:
        _wait(lambda: endpoint_path.exists() or host.poll() is not None)
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
        _wait(marker.exists)
        assert list(endpoint_path.parent.glob(f".{endpoint_path.stem}.input-*"))
        host.kill()
        host.wait(timeout=10)
        if client.poll() is None:
            client.kill()
        client.wait(timeout=10)
        assert not target.exists()
        assert list(endpoint_path.parent.glob(f".{endpoint_path.stem}.input-*"))
        host = _start(instance, tmp_path)

        def restarted() -> bool:
            current = read_endpoint(endpoint_path)
            return current is not None and current["secret"] != previous["secret"]

        _wait(restarted)
        assert not list(endpoint_path.parent.glob(f".{endpoint_path.stem}.input-*"))
        assert _command(instance, "journal", "list").returncode == 0
    finally:
        if client is not None and client.poll() is None:
            client.kill()
            client.wait(timeout=10)
        _stop(host)
