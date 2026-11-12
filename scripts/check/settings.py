"""检查工具的配置和路径。"""

from __future__ import annotations

import os
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import cast

ROOT = Path(__file__).resolve().parents[2]
STEPS = ("length", "format", "lint", "types", "tests")
LANGUAGES = ("python", "rust")


class CheckError(Exception):
    def __init__(self, message: str) -> None:
        super().__init__(message)
        self.message = message


@dataclass(frozen=True)
class LengthSpec:
    language: str
    warn: int
    reject: int
    suffix: str
    roots: tuple[str, ...]
    input: str
    format: tuple[str, ...]


@dataclass(frozen=True)
class ToolSpec:
    argv: tuple[str, ...]
    paths: tuple[str, ...] = ()
    subset: tuple[str, ...] = ()


@dataclass(frozen=True)
class RustTest:
    package: str
    argv: tuple[str, ...]


@dataclass(frozen=True)
class Profile:
    groups: tuple[str, ...]
    languages: tuple[str, ...]
    empty: str


@dataclass
class Config:
    exclude_roots: frozenset[str]
    exclude_dirs: frozenset[str]
    length: dict[str, LengthSpec]
    format: dict[str, ToolSpec]
    lint: ToolSpec
    types: ToolSpec
    pytest: ToolSpec
    pytest_full: tuple[str, ...]
    rust_tests: tuple[RustTest, ...]
    groups: dict[str, tuple[str, ...]]
    profiles: dict[str, Profile]


def within(path: Path, parent: Path) -> bool:
    try:
        child = os.path.normcase(str(path))
        base = os.path.normcase(str(parent))
        return os.path.commonpath((child, base)) == base
    except ValueError:
        return False


def display(path: Path) -> str:
    return path.resolve().relative_to(ROOT).as_posix()


def excluded(path: Path, config: Config) -> bool:
    rel = path.resolve().relative_to(ROOT)
    if rel.parts and rel.parts[0] in config.exclude_roots:
        return True
    return any(part in config.exclude_dirs for part in rel.parts)


def venv_python() -> Path:
    env = os.environ.get("UV_PROJECT_ENVIRONMENT")
    base = Path(env) if env else ROOT / "temp" / "venvs" / "nous"
    if os.name == "nt":
        return base / "Scripts" / "python.exe"
    return base / "bin" / "python"


def _list(value: object, label: str) -> list[object]:
    if not isinstance(value, list):
        raise CheckError(f"{label} 必须是数组")
    return cast(list[object], value)


def _strings(value: object, label: str, *, allow_empty: bool) -> tuple[str, ...]:
    if value is None and allow_empty:
        return ()
    kind = "字符串数组" if allow_empty else "非空字符串数组"
    items: list[str] = []
    for item in _list(value, label):
        if not isinstance(item, str):
            raise CheckError(f"{label} 必须是{kind}")
        items.append(item)
    if not allow_empty and not items:
        raise CheckError(f"{label} 必须是非空字符串数组")
    return tuple(items)


def _table(value: object, label: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise CheckError(f"{label} 必须是表")
    result: dict[str, object] = {}
    for key, item in cast(dict[object, object], value).items():
        if not isinstance(key, str):
            raise CheckError(f"{label} 的键必须是字符串")
        result[key] = item
    return result


def _length(language: str, table: object) -> LengthSpec:
    data = _table(table, f"length.{language}")
    warn = data.get("warn")
    reject = data.get("reject")
    if not isinstance(warn, int) or not isinstance(reject, int) or not 0 < warn < reject:
        raise CheckError(f"length.{language} 的 warn 必须小于 reject，且都是正整数")
    mode = data.get("input")
    if not isinstance(mode, str) or mode not in ("stdin", "path"):
        raise CheckError(f"length.{language}.input 必须是 stdin 或 path")
    suffix = data.get("suffix")
    if not isinstance(suffix, str) or not suffix.startswith("."):
        raise CheckError(f"length.{language}.suffix 必须是扩展名")
    command = _strings(data.get("format"), f"length.{language}.format", allow_empty=False)
    if mode == "path" and not any("{path}" in arg for arg in command):
        raise CheckError(f"length.{language}.format 必须包含 {{path}}")
    roots = _strings(data.get("roots"), f"length.{language}.roots", allow_empty=False)
    return LengthSpec(language, warn, reject, suffix, roots, mode, command)


def _tool(data: dict[str, object], label: str, *, subset: bool = False) -> ToolSpec:
    return ToolSpec(
        _strings(data.get("argv"), f"{label}.argv", allow_empty=False),
        _strings(data.get("paths"), f"{label}.paths", allow_empty=True),
        _strings(data.get("subset"), f"{label}.subset", allow_empty=not subset),
    )


def load_config(path: Path) -> Config:
    if not path.is_file():
        raise CheckError(f"找不到配置：{path}")
    try:
        raw = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as exc:
        raise CheckError(f"无法读取配置：{exc}") from exc
    data = _table(raw, "配置")
    length_table = _table(data.get("length"), "length")
    format_table = _table(data.get("format"), "format")
    groups_value = data.get("groups")
    group_items = [] if groups_value is None else _list(groups_value, "groups")
    groups: dict[str, tuple[str, ...]] = {}
    for item in group_items:
        group = _table(item, "groups")
        name = group.get("id")
        if not isinstance(name, str):
            raise CheckError("groups 的每一项都要有 id")
        steps = _strings(group.get("steps"), f"groups.{name}.steps", allow_empty=False)
        unknown = [step for step in steps if step not in STEPS]
        if unknown:
            raise CheckError(f"未知检查项：{unknown[0]}")
        groups[name] = steps
    profiles_table = _table(data.get("profiles"), "profiles")
    if not profiles_table:
        raise CheckError("配置缺少 profiles")
    profiles: dict[str, Profile] = {}
    for name, item in profiles_table.items():
        profile = _table(item, f"profiles.{name}")
        chosen = _strings(profile.get("groups"), f"profiles.{name}.groups", allow_empty=False)
        missing = [group for group in chosen if group not in groups]
        if missing:
            raise CheckError(f"未知检查组：{missing[0]}")
        languages = _strings(
            profile.get("languages"), f"profiles.{name}.languages", allow_empty=False
        )
        if any(language not in LANGUAGES for language in languages):
            raise CheckError(f"profiles.{name}.languages 含有未知语言")
        empty = profile.get("empty", "tree")
        if not isinstance(empty, str) or empty not in ("tree", "pass"):
            raise CheckError(f"profiles.{name}.empty 必须是 tree 或 pass")
        profiles[name] = Profile(chosen, languages, empty)
    lint = _table(data.get("lint"), "lint")
    types = _table(data.get("types"), "types")
    tests = _table(data.get("tests"), "tests")
    lint_python = _table(lint.get("python"), "lint.python")
    types_python = _table(types.get("python"), "types.python")
    tests_python = _table(tests.get("python"), "tests.python")
    rust_value = tests.get("rust")
    rust_items = [] if rust_value is None else _list(rust_value, "tests.rust")
    rust_tests: list[RustTest] = []
    for item in rust_items:
        rust = _table(item, "tests.rust")
        package = rust.get("package")
        if not isinstance(package, str):
            raise CheckError("tests.rust 的每一项都要有 package")
        rust_tests.append(
            RustTest(package, _strings(rust.get("argv"), "tests.rust.argv", allow_empty=False))
        )
    return Config(
        frozenset(_strings(data.get("exclude_roots"), "exclude_roots", allow_empty=True)),
        frozenset(_strings(data.get("exclude_dirs"), "exclude_dirs", allow_empty=True)),
        {language: _length(language, length_table.get(language)) for language in LANGUAGES},
        {
            "python": _tool(_table(format_table.get("python"), "format.python"), "format.python"),
            "rust": _tool(
                _table(format_table.get("rust"), "format.rust"), "format.rust", subset=True
            ),
        },
        _tool(lint_python, "lint.python"),
        _tool(types_python, "types.python"),
        _tool(tests_python, "tests.python"),
        _strings(tests_python.get("full"), "tests.python.full", allow_empty=False),
        tuple(rust_tests),
        groups,
        profiles,
    )
