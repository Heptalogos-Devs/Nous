"""统计指定文件或配置根目录中的有效代码行。"""

from __future__ import annotations

import os
import tokenize
from collections.abc import Sequence
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from functools import partial
from pathlib import Path

from metrics import python_code_lines, rust_code_lines
from process import command_text, emit, executable, expand, run_argv
from settings import ROOT, CheckError, Config, LengthSpec, display, excluded


@dataclass
class FileLength:
    path: Path
    count: int | None
    state: str
    limit: int | None
    detail: str


def collect(spec: LengthSpec, config: Config, selected: list[Path] | None) -> list[Path]:
    if selected is not None:
        return [
            path for path in selected if path.suffix == spec.suffix and not excluded(path, config)
        ]
    found: list[Path] = []
    for root_name in spec.roots:
        start = ROOT / root_name
        if not start.exists() or excluded(start, config):
            continue
        if start.is_file():
            if start.suffix == spec.suffix:
                found.append(start)
            continue
        for directory, dirnames, filenames in os.walk(start):
            current = Path(directory)
            dirnames[:] = [
                name
                for name in dirnames
                if not excluded(current / name, config) and not (current / name).is_symlink()
            ]
            for name in filenames:
                path = current / name
                if (
                    path.suffix == spec.suffix
                    and not path.is_symlink()
                    and not excluded(path, config)
                ):
                    found.append(path)
    return sorted(found)


def _detail(detail: str) -> tuple[str, list[str]]:
    lines = detail.splitlines() or ["无法统计"]
    return lines[0], lines[1:]


def report_length(items: list[FileLength], selected: bool) -> tuple[str, int]:
    items.sort(key=lambda item: display(item.path))
    warnings = [item for item in items if item.state == "warn"]
    rejects = [item for item in items if item.state == "reject"]
    if selected:
        for item in items:
            if item.count is None:
                head, rest = _detail(item.detail)
                print(f"{display(item.path)}  拒绝  {head}")
                for line in rest:
                    print(f"    {line}")
                continue
            if item.state == "pass":
                print(f"{display(item.path)}  {item.count}  通过")
            else:
                word = "警告" if item.state == "warn" else "拒绝"
                print(f"{display(item.path)}  {item.count}  {word}  > {item.limit}")
    if rejects:
        emit("length", "拒绝", str(len(rejects)))
        if not selected:
            for item in rejects:
                if item.count is None:
                    head, rest = _detail(item.detail)
                    print(f"  {display(item.path)}  {head}")
                    for line in rest:
                        print(f"    {line}")
                else:
                    print(f"  {display(item.path)}  {item.count} > {item.limit}")
    if warnings:
        emit("length", "警告", str(len(warnings)))
        if not selected:
            for item in warnings:
                print(f"  {display(item.path)}  {item.count} > {item.limit}")
    if not rejects and not warnings:
        emit("length", "通过")
    return ("reject" if rejects else "pass"), len(warnings)


def count_file(spec: LengthSpec, path: Path) -> FileLength:
    try:
        raw = path.read_bytes()
        raw.decode("utf-8")
    except (OSError, UnicodeError) as exc:
        return FileLength(path, None, "reject", None, f"无法读取：{exc}")
    try:
        argv = expand(spec.format, display(path))
    except CheckError as exc:
        return FileLength(path, None, "reject", None, exc.message)
    result = run_argv(argv, raw if spec.input == "stdin" else None)
    if result.returncode != 0:
        text = command_text(result)
        detail = "无法格式化" if not text else f"无法格式化\n{text}"
        return FileLength(path, None, "reject", None, detail)
    try:
        formatted = result.stdout.decode("utf-8")
        count = (
            python_code_lines(formatted)
            if spec.language == "python"
            else rust_code_lines(formatted)
        )
    except (UnicodeError, SyntaxError, tokenize.TokenError, ValueError) as exc:
        return FileLength(path, None, "reject", None, f"无法统计：{exc}")
    if count > spec.reject:
        return FileLength(path, count, "reject", spec.reject, "")
    if count > spec.warn:
        return FileLength(path, count, "warn", spec.warn, "")
    return FileLength(path, count, "pass", None, "")


def length_step(
    config: Config, languages: Sequence[str], selected: list[Path] | None
) -> tuple[str, int]:
    items: list[FileLength] = []
    for language in languages:
        spec = config.length[language]
        paths = collect(spec, config, selected)
        if not paths:
            continue
        if not executable(spec.format[0]):
            for path in paths:
                items.append(
                    FileLength(path, None, "reject", None, f"找不到命令：{spec.format[0]}")
                )
            continue
        workers = min(8, len(paths))
        with ThreadPoolExecutor(max_workers=workers) as pool:
            items.extend(pool.map(partial(count_file, spec), paths))
    return report_length(items, selected is not None)
