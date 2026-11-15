"""A persistent local host owns locks while real CLI processes share its services."""

from __future__ import annotations

import asyncio
import json
import os
import signal
import socket
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import pytest

from nous.common.errors import NousError
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.runtime.instance import InstanceLocator
from nous.transports.cli.control_wire import encode, endpoint, read_endpoint, receive_sync
from tests.cli.shared.host import command, initialize, wait_for


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
    asyncio.run(initialize(instance))

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
        wait_for(lambda: path.exists() or host.poll() is not None)
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

        from nous.application.inputs import FileInput
        from nous.transports.cli.local_control import request_call

        missing = {"unused": FileInput(tmp_path / "not-present")}
        with pytest.raises(NousError) as unknown:
            request_call(InstanceLocator(instance), "unknown.operation", {}, inputs=missing)
        assert unknown.value.code == "operation.unknown"
        with pytest.raises(NousError) as invalid:
            request_call(
                InstanceLocator(instance),
                "records.read",
                {"target": "record", "unexpected": True},
                inputs=missing,
            )
        assert invalid.value.code == "input.invalid"

        async def locked() -> None:
            with pytest.raises(NousError, match="already in use"):
                async with open_instance(instance, environment={}):
                    pass

        asyncio.run(locked())
        configured = command(instance, "config", "show", "--effective", env=client_env)
        assert configured.returncode == 0, (configured.stdout, configured.stderr)
        assert json.loads(configured.stdout)["values"]["logging"]["level"] == "DEBUG"
        explained = command(instance, "config", "explain", "logging.level", env=client_env)
        assert json.loads(explained.stdout)["effective_value"] == "DEBUG"
        models = command(instance, "tools", "model", "list", env=client_env)
        assert json.loads(models.stdout)["models"]["reader"]["model_name"] == "host-model"
        prompt = command(
            instance, "tools", "prompt", "describe", "summarize", "--task", "read", env=client_env
        )
        assert (
            json.loads(prompt.stdout)["assignments"][0]["models"][0]["configuration"]["model_name"]
            == "host-model"
        )
        parsed_time = command(instance, "time", "parse", "2026-11-03", env=client_env)
        assert json.loads(parsed_time.stdout)["timezone"] == "Asia/Shanghai"
        noop_editor = tmp_path / "noop_editor.py"
        noop_editor.write_text("pass\n", encoding="utf-8")
        edit_env = {**client_env, "EDITOR": f'"{sys.executable}" "{noop_editor}"'}
        prompt_edit = command(instance, "tools", "prompt", "edit", "summarize", env=edit_env)
        assert prompt_edit.returncode == 0, (prompt_edit.stdout, prompt_edit.stderr)
        prompt_directory = Path(json.loads(prompt_edit.stdout)["directory"])
        assert "host-model" in (prompt_directory / "CONTEXT.md").read_text(encoding="utf-8")
        invalid_client_env = {**client_env, "NOUS__LOGGING__LEVEL": "invalid"}
        assert command(instance, "config", "validate", env=invalid_client_env).returncode == 0
        source = tmp_path / "relative.md"
        source.write_text("alpha original body", encoding="utf-8")
        created = command(
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
        revision = json.loads(created.stdout)["commit"]["revision_ref"]
        ref = revision["record_ref"]["identifier"]
        queried = command(instance, "query", "alpha")
        assert queried.returncode == 0 and "alpha original body" in queried.stdout
        editor = tmp_path / "editor.py"
        editor.write_text(
            "import pathlib,sys\np=pathlib.Path(sys.argv[1]);p.write_text(p.read_text()+' edited_via_client')\n",
            encoding="utf-8",
        )
        editor.chmod(0o700)
        env = {**os.environ, "EDITOR": f'"{sys.executable}" "{editor}"'}
        edited = command(instance, "journal", "edit", ref, env=env)
        assert edited.returncode == 0, edited.stderr
        read = command(instance, "read", ref)
        assert read.returncode == 0 and "edited_via_client" in read.stdout

        def add_parallel(index: int) -> subprocess.CompletedProcess[str]:
            return command(instance, "note", "add", "--remember", data=f"parallel note {index}")

        with ThreadPoolExecutor(max_workers=4) as pool:
            results = list(pool.map(add_parallel, range(4)))
        assert all(result.returncode == 0 for result in results), [r.stderr for r in results]
        check = command(instance, "query", "parallel")
        assert check.returncode == 0 and "parallel note" in check.stdout
        previous_endpoint = read_endpoint(path)
        assert previous_endpoint is not None
        host.kill()
        host.wait(timeout=20)
        assert path.exists()  # A crash can leave only the private endpoint file.
        offline_config = command(instance, "config", "show", "--effective", env=client_env)
        assert json.loads(offline_config.stdout)["values"]["logging"]["level"] == "ERROR"
        offline_models = command(instance, "tools", "model", "list", env=client_env)
        assert json.loads(offline_models.stdout)["models"]["reader"]["model_name"] == "client-model"
        offline_prompt = command(
            instance, "tools", "prompt", "describe", "summarize", "--task", "read", env=client_env
        )
        assert (
            json.loads(offline_prompt.stdout)["assignments"][0]["models"][0]["configuration"][
                "model_name"
            ]
            == "client-model"
        )
        offline_time = command(instance, "time", "parse", "2026-11-03", env=client_env)
        assert json.loads(offline_time.stdout)["timezone"] == "America/Los_Angeles"
        package_prompt = Path(__file__).resolve().parents[3] / "src/nous/models/resources/summarize"
        imported = command(
            instance, "tools", "prompt", "import", "summarize", str(package_prompt), env=client_env
        )
        assert imported.returncode == 0, (imported.stdout, imported.stderr)
        assert "client-model" in (prompt_directory / "CONTEXT.md").read_text(encoding="utf-8")
        assert command(instance, "config", "validate", env=invalid_client_env).returncode != 0
        offline = command(instance, "journal", "list")
        assert offline.returncode == 0 and "Local host" in offline.stdout
        host = _host(instance)

        def restarted() -> bool:
            current = read_endpoint(path)
            return host.poll() is not None or (
                current is not None and current["secret"] != previous_endpoint["secret"]
            )

        wait_for(restarted)
        assert host.poll() is None
        restored = command(instance, "query", "edited_via_client")
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
    asyncio.run(initialize(instance))
    extra = f"""
original_add = RecordOperations.add_note
from functools import wraps
from nous.content.contracts import InlineText
@wraps(original_add)
async def delayed(self, request, call):
    if isinstance(request.body, InlineText) and request.body.text == "cancel-me":
        Path({str(started)!r}).write_text("started")
        try:
            await asyncio.sleep(60)
        finally:
            Path({str(ended)!r}).write_text("cancelled")
    return await original_add(self, request, call)
RecordOperations.add_note = delayed
"""
    path = endpoint(InstanceLocator(instance))
    assert path is not None
    host = _host(instance, extra)
    client = None
    try:
        wait_for(lambda: path.exists() or host.poll() is not None)
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
        wait_for(started.exists)
        client.kill()
        client.wait(timeout=10)
        wait_for(ended.exists)
        result = command(instance, "note", "add", "--text", "after cancellation")
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


def test_waiting_clients_do_not_block_background_cancellation(tmp_path: Path) -> None:
    from pydantic import TypeAdapter

    from nous.runtime.execution import OperationContext, RunningOperation
    from nous.runtime.operations import CANCEL_OPERATION, WAIT_OPERATION, SelectOperation
    from nous.transports.cli.local_control import LocalControl, request_call

    async def scenario() -> None:
        instance = tmp_path / "instance"
        locator = InstanceLocator(instance)
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}, allow_background=True) as runtime:
            control = LocalControl(runtime, locator)
            await control.start()
            started, release = asyncio.Event(), asyncio.Event()

            async def work(_context: OperationContext) -> None:
                started.set()
                await release.wait()

            ref = runtime.execution.spawn("test.wait", work, target_refs=())
            await asyncio.wait_for(started.wait(), 5)
            waiters = [
                asyncio.create_task(
                    asyncio.to_thread(
                        request_call, locator, WAIT_OPERATION.name, SelectOperation(ref=ref)
                    )
                )
                for _ in range(16)
            ]
            try:
                async with asyncio.timeout(15):
                    while len(control.handlers) < 16 or control.handshakes:  # noqa: ASYNC110
                        await asyncio.sleep(0.01)
                result = await asyncio.wait_for(
                    asyncio.to_thread(
                        request_call, locator, CANCEL_OPERATION.name, SelectOperation(ref=ref)
                    ),
                    5,
                )
                assert result is not None
                assert (
                    TypeAdapter(RunningOperation).validate_python(result["value"]).status
                    == "cancelled"
                )
                await asyncio.gather(*waiters)
            finally:
                release.set()
                await asyncio.gather(*waiters, return_exceptions=True)
                await control.stop()

    asyncio.run(scenario())
