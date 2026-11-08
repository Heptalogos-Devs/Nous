"""A persistent local host owns locks while real CLI processes share its services."""

from __future__ import annotations

import asyncio
import json
import os
import signal
import socket
import subprocess
import sys
import time
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import pytest

from nous.common.errors import NousError
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.runtime.instance import InstanceLocator
from nous.transports.cli.control_wire import encode, endpoint, read_endpoint, receive_sync


async def _initialize(path: Path) -> None:
    await initialize_instance(path, environment={})


def _wait(predicate: Callable[[], bool], *, timeout: float = 30) -> None:
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() >= deadline:
            raise AssertionError("Timed out waiting for the local host")
        time.sleep(0.05)


def _command(
    instance: Path,
    *arguments: str,
    cwd: Path | None = None,
    data: str | None = None,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable,
            "-c",
            "from nous.transports.cli import main; main()",
            "--instance",
            str(instance),
            "--json",
            *arguments,
        ],
        cwd=cwd,
        input=data,
        text=True,
        capture_output=True,
        timeout=40,
        env=env,
    )


def _host(instance: Path, extra: str = "") -> subprocess.Popen[bytes]:
    program = f"""
import asyncio
from pathlib import Path
from nous.cognition.writing.operations import RecordOperations
from nous.runtime.host import run_host
from nous.runtime.instance import InstanceLocator
from nous.transports.cli.local_control import serve_cli
{extra}
locator = InstanceLocator(Path({str(instance)!r}))
async def app(runtime):
    await asyncio.Event().wait()
run_host(locator, app, service=lambda runtime: serve_cli(runtime, locator))
"""
    return subprocess.Popen(
        [sys.executable, "-c", program], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )


def test_cli_read_write_query_edit_and_crash_recovery(tmp_path: Path) -> None:
    instance = tmp_path / "instance"
    asyncio.run(_initialize(instance))
    path = endpoint(InstanceLocator(instance))
    assert path is not None
    host = _host(instance)
    try:
        _wait(lambda: path.exists() or host.poll() is not None)
        assert host.poll() is None
        info = read_endpoint(path)
        assert info is not None and info["host"] == "127.0.0.1"
        if os.name == "nt":
            assert info["secret"] not in path.read_text(encoding="utf-8")
        else:
            assert path.stat().st_mode & 0o777 == 0o600
            assert path.parent.stat().st_mode & 0o777 == 0o700
        with socket.create_connection(("127.0.0.1", info["port"]), timeout=5) as intruder:
            intruder.sendall(encode({"kind": "hello", "nonce": "0" * 64}))
            assert receive_sync(intruder)["kind"] == "hello"
            intruder.sendall(encode({"kind": "auth", "proof": "0" * 64}))
            assert intruder.recv(1) == b""

        async def locked() -> None:
            with pytest.raises(NousError, match="already in use"):
                async with open_instance(instance, environment={}):
                    pass

        asyncio.run(locked())
        source = tmp_path / "relative.md"
        source.write_text("alpha original body", encoding="utf-8")
        created = _command(
            instance,
            "journal",
            "create",
            "--file",
            source.name,
            "--title",
            "Local host",
            cwd=tmp_path,
        )
        assert created.returncode == 0, created.stderr
        revision = json.loads(created.stdout)["record"]["revision_ref"]
        ref = revision["record_ref"]["identifier"]
        queried = _command(instance, "query", "alpha", "--mode", "lexical")
        assert queried.returncode == 0 and "alpha original body" in queried.stdout
        editor = tmp_path / "editor.py"
        editor.write_text(
            "import pathlib,sys\np=pathlib.Path(sys.argv[1]);p.write_text(p.read_text()+' edited_via_client')\n",
            encoding="utf-8",
        )
        editor.chmod(0o700)
        env = {**os.environ, "EDITOR": f'"{sys.executable}" "{editor}"'}
        edited = _command(instance, "journal", "edit", ref, env=env)
        assert edited.returncode == 0, edited.stderr
        read = _command(instance, "read", ref)
        assert read.returncode == 0 and "edited_via_client" in read.stdout

        def add_parallel(index: int) -> subprocess.CompletedProcess[str]:
            return _command(instance, "note", "add", "--remember", data=f"parallel note {index}")

        with ThreadPoolExecutor(max_workers=4) as pool:
            results = list(pool.map(add_parallel, range(4)))
        assert all(result.returncode == 0 for result in results), [r.stderr for r in results]
        check = _command(instance, "query", "parallel", "--mode", "lexical")
        assert check.returncode == 0 and "parallel note" in check.stdout
        previous_endpoint = read_endpoint(path)
        assert previous_endpoint is not None
        host.kill()
        host.wait(timeout=20)
        assert path.exists()  # A crash can leave only the private endpoint file.
        offline = _command(instance, "journal", "list")
        assert offline.returncode == 0 and "Local host" in offline.stdout
        host = _host(instance)

        def restarted() -> bool:
            current = read_endpoint(path)
            return host.poll() is not None or (
                current is not None and current["secret"] != previous_endpoint["secret"]
            )

        _wait(restarted)
        assert host.poll() is None
        restored = _command(instance, "query", "edited_via_client", "--mode", "lexical")
        assert restored.returncode == 0 and "edited_via_client" in restored.stdout
    finally:
        if host.poll() is None:
            if os.name == "nt":
                host.terminate()
            else:
                host.send_signal(signal.SIGINT)
            host.wait(timeout=30)


def test_client_disconnect_cancels_host_operation_before_resources_close(tmp_path: Path) -> None:
    instance, started, ended = tmp_path / "instance", tmp_path / "started", tmp_path / "ended"
    asyncio.run(_initialize(instance))
    extra = f"""
original_add = RecordOperations.add_note
async def delayed(self, body, *args, **kwargs):
    if body == "cancel-me":
        Path({str(started)!r}).write_text("started")
        try:
            await asyncio.sleep(60)
        finally:
            Path({str(ended)!r}).write_text("cancelled")
    return await original_add(self, body, *args, **kwargs)
RecordOperations.add_note = delayed
"""
    path = endpoint(InstanceLocator(instance))
    assert path is not None
    host = _host(instance, extra)
    client = None
    try:
        _wait(lambda: path.exists() or host.poll() is not None)
        assert host.poll() is None
        client = subprocess.Popen(
            [
                sys.executable,
                "-c",
                "from nous.transports.cli import main; main()",
                "--instance",
                str(instance),
                "note",
                "add",
                "--text",
                "cancel-me",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        _wait(started.exists)
        client.kill()
        client.wait(timeout=10)
        _wait(ended.exists)
        result = _command(instance, "note", "add", "--text", "after cancellation")
        assert result.returncode == 0, result.stderr
    finally:
        if client is not None and client.poll() is None:
            client.kill()
            client.wait(timeout=10)
        if host.poll() is None:
            if os.name == "nt":
                host.terminate()
            else:
                host.send_signal(signal.SIGINT)
            host.wait(timeout=30)


def test_windows_capability_uses_user_scoped_dpapi() -> None:
    if os.name != "nt":
        pytest.skip("Windows DPAPI is unavailable on this platform")
    from nous.transports.cli.control_windows import protect, unprotect

    secret = "af" * 32
    encrypted = protect(secret)
    assert secret not in encrypted
    assert unprotect(encrypted) == secret
    with pytest.raises(NousError):
        unprotect("not-a-valid-protected-value")
