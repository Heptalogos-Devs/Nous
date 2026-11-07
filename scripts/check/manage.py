"""开发检查入口：按配置的分组运行长度、格式、静态检查和测试。"""

from __future__ import annotations

import argparse
import sys
from collections.abc import Sequence
from pathlib import Path

from length import length_step
from process import command_text, configure_stdio, emit, executable, expand, run_argv
from settings import (
    ROOT,
    STEPS,
    CheckError,
    Config,
    Profile,
    ToolSpec,
    display,
    excluded,
    load_config,
    within,
)


def _matching(paths: list[Path] | None, suffix: str) -> list[str] | None:
    if paths is None:
        return None
    return [display(path) for path in paths if path.suffix == suffix]


def _run_tool(name: str, argv: Sequence[str]) -> str:
    try:
        prepared = expand(argv)
    except CheckError as exc:
        emit(name, "拒绝")
        print(exc.message)
        return "reject"
    if not executable(prepared[0]):
        emit(name, "拒绝")
        print(f"找不到命令：{prepared[0]}")
        return "reject"
    result = run_argv(prepared)
    emit(name, "通过" if result.returncode == 0 else "拒绝")
    if result.returncode != 0:
        text = command_text(result)
        if text:
            print(text)
        return "reject"
    return "pass"


def _format_step(config: Config, languages: Sequence[str], selected: list[Path] | None) -> str:
    failed: list[str] = []
    ran = False
    for language in languages:
        spec = config.format[language]
        if selected is None:
            argv = [*spec.argv, *spec.paths]
        else:
            files = _matching(selected, config.length[language].suffix) or []
            if not files:
                continue
            argv = [*(spec.subset if language == "rust" else spec.argv), *files]
        ran = True
        try:
            prepared = expand(argv)
        except CheckError as exc:
            failed.append(exc.message)
            continue
        if not executable(prepared[0]):
            failed.append(f"找不到命令：{prepared[0]}")
            continue
        result = run_argv(prepared)
        if result.returncode != 0:
            failed.append(command_text(result))
    emit("format", "拒绝" if failed else "通过")
    for text in failed:
        if text:
            print(text)
    if not ran and not failed:
        return "pass"
    return "reject" if failed else "pass"


def _targeted(name: str, spec: ToolSpec, suffix: str, selected: list[Path] | None) -> str:
    if selected is None:
        return _run_tool(name, [*spec.argv, *spec.paths])
    files = _matching(selected, suffix) or []
    if not files:
        emit(name, "通过")
        return "pass"
    return _run_tool(name, [*spec.argv, *files])


def _test_commands(
    config: Config,
    languages: Sequence[str],
    mode: str,
    pytest_args: Sequence[str],
    packages: Sequence[str],
) -> list[list[str]]:
    commands: list[list[str]] = []
    if "python" in languages and (mode == "all" or pytest_args):
        extra = list(config.pytest_full) if mode == "all" else list(pytest_args)
        commands.append([*config.pytest.argv, *extra])
    if "rust" in languages:
        chosen = (
            config.rust_tests
            if mode == "all"
            else tuple(item for item in config.rust_tests if item.package in packages)
        )
        commands.extend(list(item.argv) for item in chosen)
    return commands


def _tests_step(
    config: Config,
    languages: Sequence[str],
    mode: str,
    pytest_args: Sequence[str],
    packages: Sequence[str],
) -> str:
    known = {item.package for item in config.rust_tests}
    unknown = [package for package in packages if package not in known]
    if unknown:
        raise CheckError(f"未知的 Rust 测试包：{unknown[0]}")
    failed: list[str] = []
    for argv in _test_commands(config, languages, mode, pytest_args, packages):
        try:
            prepared = expand(argv)
        except CheckError as exc:
            failed.append(exc.message)
            continue
        if not executable(prepared[0]):
            failed.append(f"找不到命令：{prepared[0]}")
            continue
        result = run_argv(prepared)
        if result.returncode != 0:
            failed.append(command_text(result))
    emit("tests", "拒绝" if failed else "通过")
    for text in failed:
        if text:
            print(text)
    return "reject" if failed else "pass"


def _resolve(value: str) -> Path:
    raw = Path(value)
    if raw.is_absolute():
        path = raw
    else:
        from_cwd = (Path.cwd() / raw).resolve()
        path = from_cwd if from_cwd.exists() else (ROOT / raw).resolve()
    path = path.resolve()
    if not within(path, ROOT):
        raise CheckError(f"路径不在仓库内：{value}")
    if not path.is_file():
        raise CheckError(f"不是文件：{value}")
    return path


def _supplied(args: argparse.Namespace) -> list[str] | None:
    if args.action in ("tests", "all"):
        return None
    if args.stdin:
        if args.paths:
            raise CheckError("指定了 --stdin 时不要再传入路径")
        data = sys.stdin.buffer.read()
        separator = b"\0" if args.zero else b"\n"
        try:
            return [item.decode("utf-8") for item in data.split(separator) if item]
        except UnicodeError as exc:
            raise CheckError(f"路径列表不是 UTF-8：{exc}") from exc
    if args.paths:
        return list(args.paths)
    return None


def _eligible(paths: list[Path], config: Config, languages: Sequence[str]) -> list[Path]:
    suffixes = {config.length[language].suffix for language in languages}
    return [path for path in paths if path.suffix in suffixes and not excluded(path, config)]


def _groups(config: Config, profile: Profile, args: argparse.Namespace) -> list[tuple[str, ...]]:
    if args.action in STEPS and args.action != "tests":
        return [(args.action,)]
    names = list(profile.groups)
    wants_tests = args.action in ("tests", "all") or args.all
    if wants_tests and not any("tests" in config.groups[name] for name in names):
        if "tests" not in config.groups:
            raise CheckError("配置缺少 tests 检查组")
        names.append("tests")
    return [config.groups[name] for name in names]


def _finish(rejected: list[str], warnings: int) -> int:
    if rejected:
        print("失败：" + "、".join(rejected))
        return 1
    if warnings:
        print(f"通过，警告 {warnings}")
        return 0
    print("通过")
    return 0


def execute(config: Config, args: argparse.Namespace) -> int:
    try:
        profile = config.profiles[args.profile]
    except KeyError:
        raise CheckError(f"未知 profile：{args.profile}") from None
    if (args.all or args.action == "all") and (args.paths or args.rust):
        raise CheckError("--all 不再接受单独的测试路径或 --rust")
    groups = _groups(config, profile, args)
    flat = [step for group in groups for step in group]
    if args.action == "tests" and not args.all and not args.paths and not args.rust:
        raise CheckError("未指定测试目标。使用 tests -- <pytest 参数>、--rust <包名> 或 --all。")
    values = _supplied(args)
    if values is None and profile.empty == "pass" and args.action not in ("tests", "all"):
        print("通过")
        return 0
    selected: list[Path] | None = None
    if values is not None:
        chosen = _eligible([_resolve(value) for value in values], config, profile.languages)
        if not chosen:
            if profile.empty == "pass":
                print("通过")
                return 0
            raise CheckError("没有可检查的源码文件")
        selected = chosen
    mode = "partial" if args.action == "tests" and not args.all else "all"
    rejected: list[str] = []
    warnings = 0
    stop = False
    for group in groups:
        if stop:
            for step in group:
                emit(step, "跳过")
            continue
        group_failed = False
        for step in group:
            if step == "length":
                state, count = length_step(config, profile.languages, selected)
                warnings += count
            elif step == "format":
                state = _format_step(config, profile.languages, selected)
            elif step == "lint":
                state = _targeted("lint", config.lint, ".py", selected)
            elif step == "types":
                state = _targeted("types", config.types, ".py", selected)
            else:
                state = _tests_step(
                    config,
                    profile.languages,
                    mode if "tests" in flat else "none",
                    args.paths,
                    args.rust,
                )
            if state == "reject":
                group_failed = True
                rejected.append(step)
        if group_failed:
            stop = True
    return _finish(rejected, warnings)


def parse_args(argv: Sequence[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="check", description="检查格式、代码长度、静态检查并运行测试。"
    )
    parser.add_argument("--config", type=Path)
    parser.add_argument("--profile", default="default")
    parser.add_argument("--stdin", action="store_true")
    parser.add_argument("--zero", action="store_true")
    parser.add_argument("--all", "-All", action="store_true")
    parser.add_argument("--rust", action="append", default=[])
    parser.add_argument(
        "action", nargs="?", choices=["length", "format", "lint", "types", "tests", "all"]
    )
    parser.add_argument("paths", nargs="*")
    args = parser.parse_args(argv)
    if args.zero and not args.stdin:
        parser.error("--zero 需要和 --stdin 一起使用")
    if args.action == "all":
        args.all = True
    if args.all and args.action not in (None, "tests", "all"):
        parser.error("--all 只适用于 tests 或 all")
    if args.rust and args.action not in (None, "tests", "all"):
        parser.error("--rust 只适用于 tests 或 all")
    if args.stdin and args.action in ("tests", "all"):
        parser.error("--stdin 不能用于测试目标")
    return args


def main(argv: Sequence[str] | None = None) -> int:
    configure_stdio()
    try:
        args = parse_args(argv)
        config_path = args.config or ROOT / "scripts" / "check" / "config" / "check.toml"
        if not config_path.is_absolute():
            config_path = Path.cwd() / config_path
        return execute(load_config(config_path.resolve()), args)
    except CheckError as exc:
        print(f"错误：{exc.message}", file=sys.stderr)
        return 2
    except OSError as exc:
        print(f"错误：{exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
