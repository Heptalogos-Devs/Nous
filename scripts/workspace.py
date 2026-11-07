"""Prepare the workspace search tools and their Codex connections."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import tomlkit

ROOT = Path(__file__).resolve().parents[1]
TOOLS = ROOT / "temp" / "tools"
ZG_BIN = TOOLS / "zvec-grep" / "node_modules" / "@zvec" / "zvec-grep" / "bin"
ZG = ZG_BIN / ("zg.exe" if os.name == "nt" else "zg")
CODEGRAPH_TOOLS = "explore,node,search,callers,callees,impact"


def executable(name: str) -> str:
    path = shutil.which(name)
    if path is None:
        raise RuntimeError(f"找不到 {name}，请先安装并加入 PATH。")
    return path


def run(*args: str, env: dict[str, str] | None = None) -> None:
    print("运行:", " ".join(args), flush=True)
    subprocess.run(args, cwd=ROOT, env=env, check=True)


def install(prefix: Path, package: str) -> None:
    env = os.environ.copy()
    env["ZG_INSTALL_ACCELERATOR"] = "cpu"
    env["ZG_INSTALL_SKIP_AGENT_UPGRADE"] = "1"
    run(
        executable("npm"),
        "install",
        "--prefix",
        str(prefix),
        "--no-audit",
        "--no-fund",
        package,
        env=env,
    )


def sync_graph(graph: str) -> None:
    """Wait for an active index writer without removing its lock."""
    for attempt in range(4):
        result = subprocess.run([graph, "sync"], cwd=ROOT, capture_output=True, text=True)
        if result.returncode == 0:
            print(result.stdout, end="", flush=True)
            return
        output = result.stdout + result.stderr
        if "lock" not in output.lower() or attempt == 3:
            raise RuntimeError(f"CodeGraph 同步失败：{output.strip()}")
        print("CodeGraph 正在更新，稍候重试。", flush=True)
        time.sleep(1)


def configure_mcp(graph_command: str, graph_args: list[str], environment: dict[str, str]) -> None:
    """Update only the two managed servers, preserving other Codex settings."""
    home = Path(os.environ.get("CODEX_HOME", str(Path.home() / ".codex")))
    path = home / "config.toml"
    original = path.read_text(encoding="utf-8") if path.exists() else ""
    doc = tomlkit.parse(original) if original else tomlkit.document()
    servers = doc.setdefault("mcp_servers", tomlkit.table())
    for name, command, args, values in (
        ("codegraph", graph_command, graph_args, {"CODEGRAPH_MCP_TOOLS": CODEGRAPH_TOOLS}),
        (
            "zvec-grep",
            str(ZG),
            ["--server", "--stdio"],
            {
                "ZVEC_GREP_HOME": environment["ZVEC_GREP_HOME"],
                "ZVEC_GREP_MODEL_CACHE": environment["ZVEC_GREP_MODEL_CACHE"],
            },
        ),
    ):
        server = servers.setdefault(name, tomlkit.table())
        server.pop("url", None)
        server.pop("enabled_tools", None)
        server.pop("disabled_tools", None)
        server["command"] = command
        server["args"] = args
        server["enabled"] = True
        if name == "zvec-grep":
            server.setdefault("startup_timeout_sec", 120)
        server_env = server.setdefault("env", tomlkit.table())
        for key, value in values.items():
            server_env[key] = value
    home.mkdir(parents=True, exist_ok=True)
    updated = tomlkit.dumps(doc)
    if updated != original:
        with tempfile.NamedTemporaryFile(
            mode="w", encoding="utf-8", dir=home, delete=False
        ) as temp:
            temporary = Path(temp.name)
            temp.write(updated)
        try:
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)
    print("Codex MCP 已配置。", flush=True)


def init() -> None:
    graph = shutil.which("codegraph")
    local_bin = TOOLS / "codegraph" / "node_modules" / ".bin"
    local_graph = local_bin / ("codegraph.cmd" if os.name == "nt" else "codegraph")
    if not graph and not local_graph.exists():
        install(TOOLS / "codegraph", "@colbymchenry/codegraph@1.6.2")
    graph = graph or str(local_graph)
    if not ZG.is_file():
        install(TOOLS / "zvec-grep", "@zvec/zvec-grep@0.3.0")
    environment = os.environ.copy()
    environment["ZVEC_GREP_HOME"] = str(TOOLS / "zvec-grep" / "runtime")
    environment["ZVEC_GREP_MODEL_CACHE"] = str(TOOLS / "zvec-grep" / "models")
    # Let CodeGraph select its platform-specific index directory, including WSL.
    print("准备 CodeGraph 索引。", flush=True)
    subprocess.run([graph, "init"], cwd=ROOT, stdin=subprocess.DEVNULL, check=True)
    sync_graph(graph)
    run(
        str(ZG),
        "--index",
        str(ROOT),
        "--mode",
        "direct",
        "--embedding",
        "local/potion-retrieval-32m",
        "--model-cache",
        environment["ZVEC_GREP_MODEL_CACHE"],
        "--glob",
        "!legacy/**",
        env=environment,
    )
    executable("codex")
    # An installed npm shim is resolved by Codex using PATH; a local install uses Node directly.
    if Path(graph) == local_graph:
        graph_command = executable("node")
        shim = TOOLS / "codegraph" / "node_modules" / "@colbymchenry" / "codegraph" / "npm-shim.js"
        graph_args = [str(shim), "serve", "--mcp"]
    else:
        graph_command, graph_args = graph, ["serve", "--mcp"]
    configure_mcp(graph_command, graph_args, environment)
    print("工作区检索工具已就绪。Codex 中重连 MCP 或重启后使用新配置。", flush=True)


def status() -> None:
    run(executable("codegraph"), "status")
    run(str(ZG), "--status", "--mode", "direct")


if __name__ == "__main__":
    try:
        if len(sys.argv) != 2 or sys.argv[1] not in ("init", "status"):
            raise RuntimeError("用法：workspace init|status")
        (init if sys.argv[1] == "init" else status)()
    except (OSError, RuntimeError, ValueError, subprocess.CalledProcessError) as error:
        print(f"工作区初始化失败：{error}", file=sys.stderr)
        sys.exit(1)
