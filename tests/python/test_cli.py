"""The CLI uses real HTTP, strict JSON and protected one-time credential files."""

import asyncio
import json
import os
import socket
import subprocess
import sys
import time
from pathlib import Path

import httpx
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client


def test_installed_default_instance_is_independent_of_cwd_and_keeps_overrides(tmp_path):
    command = [sys.executable, "-I", "-c", "from nous.transports.cli import main; main()"]
    environment = os.environ.copy()
    environment["NOUS_CLIENT_CONFIG"] = str(tmp_path / "client.toml")
    for name in ("NOUS_INSTANCE", "NOUS_CREDENTIAL_FILE"):
        environment.pop(name, None)
    environment.update(
        LOCALAPPDATA=str(tmp_path / "data"),
        XDG_DATA_HOME=str(tmp_path / "data"),
        HOME=str(tmp_path / "home"),
    )
    first, second = tmp_path / "first", tmp_path / "second"
    first.mkdir()
    second.mkdir()

    def invoke_at(directory, *arguments, env=environment):
        result = subprocess.run(
            [*command, "--json", *arguments],
            cwd=directory,
            env=env,
            check=True,
            capture_output=True,
            encoding="utf-8",
            timeout=40,
        )
        return json.loads(result.stdout)

    initialized = invoke_at(first, "init")
    expected = Path(initialized["instance"])
    assert expected.is_absolute() and expected.name == "instance"
    assert expected.is_relative_to(tmp_path)
    credential = Path(initialized["credential_file"]).read_bytes()
    assert invoke_at(second, "init") == initialized
    assert Path(initialized["credential_file"]).read_bytes() == credential
    configured = subprocess.run(
        [
            sys.executable,
            "-I",
            "-c",
            "from nous.platform.configuration import Settings; print(Settings().instance)",
        ],
        cwd=second,
        env=environment,
        check=True,
        capture_output=True,
        encoding="utf-8",
        timeout=20,
    )
    assert Path(configured.stdout.strip()) == expected
    env_instance, flag_instance = tmp_path / "environment", tmp_path / "explicit"
    overridden = {**environment, "NOUS_INSTANCE": str(env_instance)}
    assert Path(invoke_at(first, "init", env=overridden)["instance"]) == env_instance
    assert (
        Path(
            invoke_at(second, "--instance", str(flag_instance), "init", env=overridden)["instance"]
        )
        == flag_instance
    )
    assert (
        Path(
            invoke_at(second, "init", "--instance", str(flag_instance), env=overridden)["instance"]
        )
        == flag_instance
    )
    assert Path(initialized["credential_file"]).read_bytes() == credential

    # An existing installation keeps its previous data location without moving it.
    legacy_env = {
        **environment,
        "LOCALAPPDATA": str(tmp_path / "legacy-data"),
        "XDG_DATA_HOME": str(tmp_path / "legacy-data"),
        "HOME": str(tmp_path / "legacy-home"),
    }
    parent = (
        tmp_path / "legacy-home/Library/Application Support"
        if sys.platform == "darwin"
        else tmp_path / "legacy-data"
    )
    legacy = (
        parent / ("NousWave" if sys.platform in {"win32", "darwin"} else "nous-wave") / "instance"
    )
    previous = invoke_at(first, "init", "--instance", str(legacy), env=legacy_env)
    assert invoke_at(second, "init", env=legacy_env) == previous

    with socket.socket() as available:
        available.bind(("127.0.0.1", 0))
        port = available.getsockname()[1]
    url = f"http://127.0.0.1:{port}"
    with (tmp_path / "default-host.log").open("w", encoding="utf-8") as log:
        host = subprocess.Popen(
            [*command, "serve", "--port", str(port)],
            cwd=first,
            env=environment,
            stdout=log,
            stderr=log,
        )
        headers = {"Authorization": "Bearer " + credential.decode().strip()}
        try:
            deadline = time.monotonic() + 40
            while True:
                try:
                    response = httpx.get(url + "/api/manage/status", headers=headers, timeout=1)
                    if response.is_success:
                        break
                except httpx.HTTPError:
                    pass
                assert host.poll() is None and time.monotonic() < deadline
                time.sleep(0.1)
            assert Path(response.json()["instance"]) == expected
            assert Path(invoke_at(second, "--url", url, "status")["instance"]) == expected
            assert (expected / "data/authority.sqlite").is_file()
            assert not (first / "temp/instances/default").exists()
            assert not (second / "temp/instances/default").exists()
        finally:
            if host.poll() is None:
                try:
                    httpx.post(url + "/api/manage/stop", headers=headers, timeout=5)
                    host.wait(timeout=25)
                except httpx.HTTPError, subprocess.TimeoutExpired:
                    host.terminate()
                    host.wait(timeout=10)


def test_cli_namespace_workflow_and_mcp_reads(tmp_path):
    command = [sys.executable, "-c", "from nous.transports.cli import main; main()"]
    instance = tmp_path / "instance"
    environment = {**os.environ, "NOUS_CLIENT_CONFIG": str(tmp_path / "client.toml")}
    with socket.socket() as available:
        available.bind(("127.0.0.1", 0))
        port = available.getsockname()[1]
    url = f"http://127.0.0.1:{port}"
    connection = ["--instance", str(instance), "--url", url, "--json"]

    def invoke(*arguments, credential=None, success=True):
        options = connection.copy()
        if credential is not None:
            options += ["--credential-file", str(credential)]
        response = subprocess.run(
            [*command, *options, *arguments],
            env=environment,
            capture_output=True,
            encoding="utf-8",
            timeout=60,
        )
        assert (response.returncode == 0) == success, (response.stdout, response.stderr)
        return json.loads(response.stdout)

    initialized = invoke("init")
    assert initialized["namespace"] == "default"
    document = tmp_path / "field-notes.txt"
    document.write_text(
        "NATIVE-CLI-ALPHA-73 records how import and retrieval share one namespace.",
        encoding="utf-8",
    )
    log = tmp_path / "host.log"
    with log.open("w", encoding="utf-8") as diagnostics:
        host = subprocess.Popen(
            [*command, "--instance", str(instance), "--url", url, "serve", "--port", str(port)],
            env=environment,
            stdout=diagnostics,
            stderr=diagnostics,
        )
        headers = {
            "Authorization": "Bearer "
            + (instance / "credentials" / "admin.token").read_text(encoding="utf-8").strip()
        }
        try:
            deadline = time.monotonic() + 40
            while True:
                try:
                    response = httpx.get(
                        url + "/api/manage/status", headers=headers, timeout=1
                    )
                    if response.is_success:
                        break
                except httpx.HTTPError:
                    pass
                assert host.poll() is None and time.monotonic() < deadline, log.read_text(
                    encoding="utf-8"
                )
                time.sleep(0.1)

            selected = invoke("use", "default")
            namespace_id = selected["ref"].removeprefix("namespace:")
            imported = invoke("import", str(document), "--no-formation")
            assert imported["status"] == "completed" and imported["materials"]

            found = invoke("query", "NATIVE-CLI-ALPHA-73")
            short_ref = found["items"][0]["ref"]
            assert short_ref and len(short_ref) <= 24 and "@" not in short_ref
            read = invoke("read", short_ref)
            assert "NATIVE-CLI-ALPHA-73" in read["text"]

            memory = invoke(
                "remember", "The service keeps each write in one atomic transaction."
            )
            corrected = invoke(
                "correct",
                memory["ref"],
                "The service commits each accepted write atomically.",
                "--reason",
                "Use the current transaction contract.",
            )
            assert corrected["text"] == "Correction saved."
            work = invoke(
                "work",
                "start",
                "Review the transaction contract",
                "--text",
                "Confirm the updated wording in the next implementation pass.",
                "--anchor",
                memory["ref"],
            )
            assert work["ref"].startswith("work:")

            async def mcp_protocol():
                parameters = StdioServerParameters(
                    command=command[0],
                    env=environment,
                    args=[
                        *command[1:],
                        *connection,
                        "--namespace",
                        namespace_id,
                        "mcp",
                    ],
                )
                async with stdio_client(
                    parameters, errlog=sys.stderr
                ) as (reader, writer):
                    async with ClientSession(
                        reader, writer, read_timeout_seconds=30
                    ) as session:
                        initialized = await session.initialize()
                        assert initialized.server_info.name == "Nous"
                        tools = {tool.name for tool in (await session.list_tools()).tools}
                        assert {"nous_query", "nous_read", "nous_correct", "nous_work_open"} <= tools
                        result = await session.call_tool(
                            "nous_query", {"query": "NATIVE-CLI-ALPHA-73"}
                        )
                        assert not result.is_error
                        reference = result.structured_content["items"][0]["ref"]
                        assert len(reference) <= 24 and "@" not in reference
                        detail = await session.call_tool("nous_read", {"ref": reference})
                        assert not detail.is_error
                        assert "NATIVE-CLI-ALPHA-73" in detail.structured_content["text"]
                        followup = await session.call_tool(
                            "nous_work_open",
                            {
                                "goal": "Recheck imported notes",
                                "background": "The note came from the CLI import.",
                                "anchors": [reference],
                            },
                        )
                        assert not followup.is_error
                        assert followup.structured_content["ref"].startswith("work:")

            asyncio.run(mcp_protocol())

            archive = tmp_path / "knowledge.json"
            invoke("bundle", "export", str(archive))
            invoke("namespace", "create", "bundle-target")
            invoke("use", "bundle-target")
            invoke("bundle", "install", str(archive))
            imported_memory = invoke("read", memory["ref"], "--sources")
            assert imported_memory["text"] == "The service keeps each write in one atomic transaction."
            revised_ref = next(item["ref"] for item in corrected["items"] if item["kind"] == "memory")
            revised = invoke("read", revised_ref)
            assert revised["text"] == "The service commits each accepted write atomically."
        finally:
            if host.poll() is None:
                try:
                    httpx.post(url + "/api/manage/stop", headers=headers, timeout=5)
                    host.wait(timeout=25)
                except (httpx.HTTPError, subprocess.TimeoutExpired):
                    host.terminate()
                    host.wait(timeout=10)
