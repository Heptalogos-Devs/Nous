"""The local runtime's user-visible path and its persistent contracts."""

import json
import os
from pathlib import Path

from nous.runtime.instance import InstanceLock
from tests.cli.shared.host import command


def test_local_cli_initialization_configuration_and_runtime(tmp_path: Path) -> None:
    instance = tmp_path / "独立 instance"
    cwd = tmp_path / "another working directory"
    cwd.mkdir()
    environment = {key: value for key, value in os.environ.items() if not key.startswith("NOUS__")}

    def invoke(*args: str, env: dict[str, str] | None = None, success: bool = True):
        result = command(instance, *args, cwd=cwd, env=env or environment)
        assert (result.returncode == 0) == success, (result.stdout, result.stderr)
        return json.loads(result.stdout)

    initialized = invoke("init", "--layout", "portable")
    identity = initialized["instance"]["instance_id"]
    assert invoke("init")["instance"]["instance_id"] == identity

    assert invoke("config", "set", "logging.level", "DEBUG")["state"] == "saved"
    invoke("config", "set", "execution.max_background_tasks", "7")
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
    assert status["state"] == "ready"
    assert status["database"]["instance_id"] == identity
    assert (
        invoke("config", "show", "--effective")["values"]["execution"]["max_background_tasks"] == 7
    )
    parsed = invoke("time", "parse", "2026-11-03", "--timezone", "Asia/Shanghai")["value"]
    assert parsed["meaning"] == "uncertain_instant" and parsed["precision"] == "day"

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
