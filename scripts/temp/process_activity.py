"""Find live filesystem users and mounts before temporary data cleanup."""

from __future__ import annotations

import json
import os
import re
import shutil
import sys
from collections.abc import Sequence
from pathlib import Path
from typing import TypedDict, Union, cast

from shared import Unsafe, command, within

ProcessReference = tuple[str, str, list[str], list[str]]
BusyPaths = tuple[list[ProcessReference], list[tuple[str, bool]], list[str]]


class ProcessInfo(TypedDict):
    ProcessId: int
    ParentProcessId: int | None
    Name: str | None
    ExecutablePath: str | None
    CommandLine: str | None


class MountInfo(TypedDict):
    Type: str
    Source: str


class ContainerInfo(TypedDict):
    Mounts: list[MountInfo]


def cleanup_invocation(arguments: Sequence[str]) -> bool:
    """Identify our own launch command without hiding unrelated ancestor references."""
    command_line = " ".join(arguments).replace("\\", "/").casefold()
    return any(
        name in command_line
        for name in (
            "scripts/temp/manage.py",
            "scripts/temp/run.ps1",
            "scripts/temp/run.sh",
        )
    )


def process_references() -> tuple[list[ProcessReference], list[str]]:
    if not sys.platform.startswith("linux") or not Path("/proc/self/status").exists():
        return [], ["当前平台无法检查进程的命令行、工作目录及打开文件"]
    processes: list[ProcessReference] = []
    errors: list[str] = []
    ancestors: set[str] = set()
    parent = os.getpid()
    while parent and str(parent) not in ancestors:
        ancestors.add(str(parent))
        try:
            status = Path(f"/proc/{parent}/status").read_text()
            match = re.search(r"^PPid:\s+(\d+)", status, re.M)
            parent = int(match[1]) if match else 0
        except OSError:
            break
    for proc in Path("/proc").iterdir():
        if not proc.name.isdigit():
            continue
        try:
            status = (proc / "status").read_text()
            if re.search(r"^State:\s+[ZX]\b|^Kthread:\s+1\b", status, re.M):
                continue
            uid = re.search(r"^Uid:\s+(\d+)", status, re.M)
            owned = uid is not None and int(uid[1]) == os.getuid()
            name_match = re.search(r"^Name:\s+(.+)", status, re.M)
            if name_match is None:
                raise ValueError("missing process name")
            name = name_match[1]
        except (OSError, TypeError, ValueError):
            try:
                if proc.stat().st_uid == os.getuid():
                    errors.append(f"无法检查当前用户进程 {proc.name}")
            except FileNotFoundError:
                pass
            continue
        paths: list[str] = []
        unreadable: list[str] = []
        arguments: list[str] = []
        cwd: str | None = None
        try:
            arguments = [
                os.fsdecode(arg) for arg in (proc / "cmdline").read_bytes().split(b"\0") if arg
            ]
        except OSError:
            unreadable.append("cmdline")
        if proc.name in ancestors and cleanup_invocation(arguments):
            # These command lines contain the user's selected paths, not a separate user of them.
            arguments = []
        for field in ("exe", "cwd"):
            try:
                raw_target = os.readlink(proc / field)
                target = raw_target.removesuffix(" (deleted)")
                paths.extend((raw_target, target))
                if field == "cwd":
                    cwd = target
            except OSError:
                unreadable.append(field)
        tool_paths = {
            "uv": ("UV_CACHE_DIR",),
            "cargo": ("CARGO_HOME", "CARGO_TARGET_DIR"),
            "rustc": ("CARGO_HOME", "CARGO_TARGET_DIR"),
            "go": ("GOCACHE", "GOMODCACHE"),
            "bun": ("BUN_INSTALL_CACHE_DIR",),
        }
        if name in tool_paths:
            try:
                entries = (proc / "environ").read_bytes().split(b"\0")
                wanted = {*tool_paths[name], "XDG_CACHE_HOME", "HOME"}
                values: dict[str, str] = {}
                for entry in entries:
                    key, separator, value = entry.partition(b"=")
                    decoded_key = os.fsdecode(key)
                    if separator and decoded_key in wanted:
                        values[decoded_key] = os.fsdecode(value)
                targets = [values[key] for key in tool_paths[name] if values.get(key)]
                if name == "uv" and not values.get("UV_CACHE_DIR"):
                    base = values.get("XDG_CACHE_HOME")
                    if not base and values.get("HOME"):
                        base = os.path.join(values["HOME"], ".cache")
                    if base:
                        targets.append(os.path.join(base, "uv"))
                for target in targets:
                    absolute = os.path.abspath(os.path.join(cwd or "/", target))
                    paths.extend((absolute, os.path.realpath(absolute)))
            except OSError:
                unreadable.append("environ")
        try:
            for descriptor in (proc / "fd").iterdir():
                try:
                    target = os.readlink(descriptor)
                    if target.startswith("/"):
                        paths.extend((target, target.removesuffix(" (deleted)")))
                except FileNotFoundError:
                    pass  # Descriptors can close while scanning.
                except OSError:
                    unreadable.append("fd")
        except OSError:
            unreadable.append("fd")
        try:
            current = (proc / "status").read_text()
            if re.search(r"^State:\s+[ZX]\b", current, re.M):
                continue
        except FileNotFoundError:
            continue
        except OSError:
            unreadable.append("status")
        if owned and unreadable:
            errors.append(
                f"无法完整检查当前用户进程 {proc.name} ({name})：{', '.join(set(unreadable))}"
            )
        if cwd:
            for arg in arguments:
                value = arg.split("=", 1)[1] if arg.startswith("-") and "=" in arg else arg
                value = value.removeprefix("@")
                if value and not value.startswith("-"):
                    lexical = os.path.abspath(os.path.join(cwd, value))
                    paths.extend((lexical, os.path.realpath(lexical)))
        processes.append((proc.name, name, paths, arguments))
    return processes, errors


def busy_paths() -> BusyPaths:
    if os.name == "nt":
        return windows_busy_paths()
    processes, errors = process_references()
    mounts: list[tuple[str, bool]] = []
    if sys.platform.startswith("linux"):
        try:
            for line in Path("/proc/self/mountinfo").read_text().splitlines():
                # mountinfo uses octal escapes, including spaces and backslashes.
                field = line.split()[4]
                mounts.append((re.sub(r"\\([0-7]{3})", lambda m: chr(int(m[1], 8)), field), False))
        except (OSError, IndexError):
            errors.append("无法检查当前挂载点")
    docker = shutil.which("docker")
    if docker:
        try:
            ids = command([docker, "ps", "--quiet"]).decode().split()
            if ids:
                for container in cast(
                    list[ContainerInfo], json.loads(command([docker, "inspect", *ids]))
                ):
                    mounts.extend(
                        (mount["Source"], True)
                        for mount in container["Mounts"]
                        if mount["Type"] == "bind"
                    )
        except (Unsafe, ValueError, KeyError, TypeError):
            errors.append("无法检查运行中的 Docker bind mount")
    return processes, mounts, errors


def argument_references_path(path: Path, argument: str) -> bool:
    """Match an argument path at component boundaries, not a sibling prefix."""
    target = os.path.normcase(str(path))
    value = os.path.normcase(argument)
    prefix = r"(?:(?<![\w./\\-])|(?<=-I)|(?<=-L)|(?<=-o)|(?<=-C)|(?<=-i)|(?<=-l)|(?<=-c))"
    suffix = r"(?=$|[\\/\s\"'=,:;)\]}])"
    pattern = prefix + re.escape(target) + suffix
    return re.search(pattern, value) is not None


def block_reason(path: Path, busy: BusyPaths) -> str | None:
    processes, mounts, errors = busy
    if within(os.getcwd(), path):
        return "当前工作目录位于其中"
    # Ordinary filesystem ancestors (/, /tmp) do not make every path busy.
    if any(within(mount, path) or (bind and within(path, mount)) for mount, bind in mounts):
        return "子树含挂载点或运行中的 Docker bind mount"
    for pid, name, paths, args in processes:
        if any(within(target, path) for target in paths) or any(
            argument_references_path(path, arg) for arg in args
        ):
            return f"进程 {pid} ({name}) 正在使用"
    return "；".join(errors) if errors else None


def windows_busy_paths() -> BusyPaths:
    processes: list[ProcessReference] = []
    mounts: list[tuple[str, bool]] = []
    errors: list[str] = []
    try:
        script = (
            "[Console]::OutputEncoding=[Text.UTF8Encoding]::new(); "
            "Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name,ExecutablePath,CommandLine | ConvertTo-Json -Compress"
        )
        raw = command(["pwsh", "-NoProfile", "-NonInteractive", "-Command", script])
        data = cast(Union[ProcessInfo, list[ProcessInfo]], json.loads(raw.decode("utf-8-sig")))  # noqa: UP007
        items = [data] if isinstance(data, dict) else data
        parents = {item["ProcessId"]: item["ParentProcessId"] for item in items}
        ancestors: set[int] = set()
        parent: int | None = os.getpid()
        while parent and parent not in ancestors:
            ancestors.add(parent)
            parent = parents.get(parent)
        for item in items:
            executable = item.get("ExecutablePath")
            line = item.get("CommandLine") or ""
            arguments = (
                [] if item["ProcessId"] in ancestors and cleanup_invocation([line]) else [line]
            )
            processes.append(
                (
                    str(item["ProcessId"]),
                    item["Name"] or "",
                    [executable] if executable else [],
                    arguments,
                )
            )
    except (Unsafe, ValueError, KeyError, TypeError):
        errors.append("无法读取 Windows CIM 进程信息")
    docker = shutil.which("docker")
    if docker:
        try:
            ids = command([docker, "ps", "--quiet"]).decode().split()
            if ids:
                for container in cast(
                    list[ContainerInfo], json.loads(command([docker, "inspect", *ids]))
                ):
                    for mount in container["Mounts"]:
                        if mount["Type"] == "bind":
                            source = re.sub(
                                r"^/(?:run/desktop/mnt/host|host_mnt|mnt)/([a-zA-Z])/",
                                r"\1:/",
                                mount["Source"],
                            )
                            mounts.append((source, True))
        except (Unsafe, ValueError, KeyError, TypeError):
            errors.append("无法检查运行中的 Docker bind mount")
    return processes, mounts, errors
