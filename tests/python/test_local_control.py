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

    async def configure() -> None:
        from nous.application.configuration import open_configuration_operations

        async with open_configuration_operations(instance, environment={}) as settings:
            await settings.update(
                {
                    "providers.local": {
                        "protocol": "openai-chat-completions",
                        "endpoint": "http://127.0.0.1:1/v1",
                    },
                    "models.reader": {
                        "provider": "local",
                        "model_name": "saved-model",
                        "capabilities": ["text"],
                    },
                    "tasks.read": {"model": "reader", "prompt": "summarize"},
                }
            )

    asyncio.run(configure())
    path = endpoint(InstanceLocator(instance))
    assert path is not None
    host = _host(
        instance,
        """
import os
os.environ['NOUS__LOGGING__LEVEL'] = 'DEBUG'
os.environ['NOUS__MODELS__READER__MODEL_NAME'] = 'host-model'
os.environ['NOUS__LOCALIZATION__TIMEZONE'] = 'Asia/Shanghai'
""",
    )
    client_env = {
        **{key: value for key, value in os.environ.items() if not key.startswith("NOUS__")},
        "NOUS__LOGGING__LEVEL": "ERROR",
        "NOUS__MODELS__READER__MODEL_NAME": "client-model",
        "NOUS__LOCALIZATION__TIMEZONE": "America/Los_Angeles",
    }
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
        configured = _command(instance, "config", "show", "--effective", env=client_env)
        assert configured.returncode == 0, (configured.stdout, configured.stderr)
        assert json.loads(configured.stdout)["values"]["logging"]["level"] == "DEBUG"
        explained = _command(instance, "config", "explain", "logging.level", env=client_env)
        assert json.loads(explained.stdout)["effective_value"] == "DEBUG"
        models = _command(instance, "tools", "model", "list", env=client_env)
        assert json.loads(models.stdout)["models"]["reader"]["model_name"] == "host-model"
        prompt = _command(
            instance, "tools", "prompt", "describe", "summarize", "--task", "read", env=client_env
        )
        assert (
            json.loads(prompt.stdout)["assignments"][0]["models"][0]["configuration"]["model_name"]
            == "host-model"
        )
        parsed_time = _command(instance, "time", "parse", "2026-11-03", env=client_env)
        assert json.loads(parsed_time.stdout)["timezone"] == "Asia/Shanghai"
        noop_editor = tmp_path / "noop_editor.py"
        noop_editor.write_text("pass\n", encoding="utf-8")
        edit_env = {**client_env, "EDITOR": f'"{sys.executable}" "{noop_editor}"'}
        prompt_edit = _command(instance, "tools", "prompt", "edit", "summarize", env=edit_env)
        assert prompt_edit.returncode == 0, (prompt_edit.stdout, prompt_edit.stderr)
        prompt_directory = Path(json.loads(prompt_edit.stdout)["directory"])
        assert "host-model" in (prompt_directory / "CONTEXT.md").read_text(encoding="utf-8")
        invalid_client_env = {**client_env, "NOUS__LOGGING__LEVEL": "invalid"}
        assert _command(instance, "config", "validate", env=invalid_client_env).returncode == 0
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
        offline_config = _command(instance, "config", "show", "--effective", env=client_env)
        assert json.loads(offline_config.stdout)["values"]["logging"]["level"] == "ERROR"
        offline_models = _command(instance, "tools", "model", "list", env=client_env)
        assert json.loads(offline_models.stdout)["models"]["reader"]["model_name"] == "client-model"
        offline_prompt = _command(
            instance, "tools", "prompt", "describe", "summarize", "--task", "read", env=client_env
        )
        assert (
            json.loads(offline_prompt.stdout)["assignments"][0]["models"][0]["configuration"][
                "model_name"
            ]
            == "client-model"
        )
        offline_time = _command(instance, "time", "parse", "2026-11-03", env=client_env)
        assert json.loads(offline_time.stdout)["timezone"] == "America/Los_Angeles"
        package_prompt = Path(__file__).resolve().parents[2] / "src/nous/models/resources/summarize"
        imported = _command(
            instance, "tools", "prompt", "import", "summarize", str(package_prompt), env=client_env
        )
        assert imported.returncode == 0, (imported.stdout, imported.stderr)
        assert "client-model" in (prompt_directory / "CONTEXT.md").read_text(encoding="utf-8")
        assert _command(instance, "config", "validate", env=invalid_client_env).returncode != 0
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
from functools import wraps
@wraps(original_add)
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
