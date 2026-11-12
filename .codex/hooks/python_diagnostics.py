"""Report syntax and type diagnostics for changed Python files without editing them."""

import ast
import hashlib
import io
import json
import os
import subprocess
import sys
import tokenize
import tomllib
from pathlib import Path, PurePosixPath
from typing import NamedTuple, cast

ROOT = Path(__file__).resolve().parents[2]
CACHE = ROOT / "temp" / "cache" / "codex-python-diagnostics"
SCRIPTS = ROOT / ".venv" / ("Scripts" if os.name == "nt" else "bin")


class GeneratedStubRule(NamedTuple):
    paths: tuple[str, ...]
    marker: bytes


def generated_stub_rules() -> tuple[GeneratedStubRule, ...]:
    try:
        document = tomllib.loads((ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    except FileNotFoundError:
        return ()
    tool = document.get("tool", {})
    if not isinstance(tool, dict):
        raise ValueError("tool must be a TOML table")
    settings = cast(dict[str, object], tool).get("codex-python-diagnostics", {})
    if not isinstance(settings, dict):
        raise ValueError("tool.codex-python-diagnostics must be a TOML table")
    configured = cast(dict[str, object], settings).get("generated-stubs", [])
    if not isinstance(configured, list):
        raise ValueError("generated-stubs must be a list of path and marker rules")
    rules: list[GeneratedStubRule] = []
    for value in cast(list[object], configured):
        if not isinstance(value, dict):
            raise ValueError("Each generated-stubs rule must be a TOML table")
        rule = cast(dict[str, object], value)
        paths, marker = rule.get("paths"), rule.get("marker")
        if (
            not isinstance(paths, list)
            or not paths
            or not all(
                isinstance(pattern, str) and pattern for pattern in cast(list[object], paths)
            )
        ):
            raise ValueError("generated-stubs paths must be a nonempty list of glob strings")
        if not isinstance(marker, str) or not marker:
            raise ValueError("generated-stubs marker must be a nonempty string")
        rules.append(GeneratedStubRule(tuple(cast(list[str], paths)), marker.encode("utf-8")))
    return tuple(rules)


def generated_stub(path: Path, rules: tuple[GeneratedStubRule, ...]) -> bool:
    if path.suffix != ".pyi":
        return False
    relative = PurePosixPath(path.relative_to(ROOT).as_posix())
    matching = [
        rule for rule in rules if any(relative.full_match(pattern) for pattern in rule.paths)
    ]
    if not matching:
        return False
    with path.open("rb") as stream:
        header = stream.read(1024)
    comments: list[str] = []
    docstring_seen = False
    try:
        for token in tokenize.tokenize(io.BytesIO(header).readline):
            if token.type in {tokenize.ENCODING, tokenize.NL, tokenize.NEWLINE}:
                continue
            if token.type == tokenize.COMMENT:
                comments.append(token.string)
            elif token.type == tokenize.STRING and not docstring_seen:
                value: object = ast.literal_eval(token.string)
                if not isinstance(value, str):
                    break
                comments.append(value)
                docstring_seen = True
            else:
                break
    except tokenize.TokenError, SyntaxError, UnicodeError:
        return False
    provenance = "\n".join(comments).encode("utf-8")
    return any(rule.marker in provenance for rule in matching)


def changed_files() -> list[Path]:
    repository = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"], cwd=ROOT, capture_output=True, check=False
    )
    if repository.returncode or Path(os.fsdecode(repository.stdout).strip()).resolve() != ROOT:
        return []
    # Separate index/worktree diffs also support repositories with no initial commit.
    commands = (
        ["git", "diff", "--name-only", "-z", "--"],
        ["git", "diff", "--cached", "--name-only", "-z", "--"],
        ["git", "ls-files", "--others", "--exclude-standard", "-z"],
    )
    paths: set[Path] = set()
    for command in commands:
        result = subprocess.run(command, cwd=ROOT, capture_output=True, check=True)
        for value in result.stdout.split(b"\0"):
            if value:
                path = ROOT / os.fsdecode(value)
                if (
                    path.suffix in {".py", ".pyi"}
                    and path.is_file()
                    and path.resolve().is_relative_to(ROOT)
                ):
                    paths.add(path)
    return sorted(paths)


def feedback(message: str) -> None:
    print(
        json.dumps(
            {"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": message}},
            ensure_ascii=False,
        )
    )


def main() -> None:
    event = json.load(sys.stdin)
    session = hashlib.sha256(event.get("session_id", "").encode()).hexdigest()
    cache = CACHE / f"{session}.json"
    previous: dict[str, str]
    try:
        previous = json.loads(cache.read_text(encoding="utf-8"))
    except OSError, ValueError:
        previous = {}
    files = changed_files()
    rules = generated_stub_rules() if files else ()
    files = [path for path in files if not generated_stub(path, rules)]
    configuration = ROOT / "pyproject.toml"
    settings_digest = hashlib.sha256(
        configuration.read_bytes() if configuration.is_file() else b""
    ).digest()
    current = {
        str(path.relative_to(ROOT)): hashlib.sha256(settings_digest + path.read_bytes()).hexdigest()
        for path in files
    }
    selected = [name for name, digest in current.items() if previous.get(name) != digest]
    if not selected:
        if current != previous:
            CACHE.mkdir(parents=True, exist_ok=True)
            cache.write_text(json.dumps(current), encoding="utf-8")
        return
    suffix = ".exe" if os.name == "nt" else ""
    ruff = SCRIPTS / ("ruff" + suffix)
    pyright = SCRIPTS / ("basedpyright" + suffix)
    if not ruff.is_file() or not pyright.is_file():
        feedback(
            "Python diagnostics unavailable: initialize the project development environment with uv sync --locked."
        )
        return
    environment = os.environ.copy()
    environment["PATH"] = str(SCRIPTS) + os.pathsep + environment.get("PATH", "")
    environment["VIRTUAL_ENV"] = str(SCRIPTS.parent)
    environment["PYTHONIOENCODING"] = "utf-8"
    selected_paths = {(ROOT / name).resolve() for name in selected}
    messages: list[str] = []
    for label, command in (
        (
            "Python syntax",
            [str(ruff), "check", "--no-cache", "--no-fix", "--select", "E9", *selected],
        ),
        ("Python types", [str(pyright), "--outputjson", *selected]),
    ):
        result = subprocess.run(
            command,
            cwd=ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            env=environment,
        )
        if label == "Python types" and result.stdout:
            try:
                report = json.loads(result.stdout)
                diagnostics = report.get("generalDiagnostics", [])
                lines: list[str] = []
                for item in diagnostics:
                    if not item.get("rule"):
                        continue
                    path = Path(item["file"]).resolve()
                    if path not in selected_paths:
                        continue
                    pos = item.get("range", {}).get("start", {})
                    path = path.relative_to(ROOT)
                    lines.append(
                        f"{path}:{pos.get('line', 0) + 1}:{pos.get('character', 0) + 1} [{item['severity']}] {item['message']}"
                    )
                if lines:
                    messages.append(label + ":\n" + "\n".join(lines[:15]))
                    if len(lines) > 15:
                        messages.append(f"{len(lines) - 15} additional type diagnostics omitted.")
                if result.returncode > 1:
                    messages.append(label + ": " + (result.stderr.strip() or "checker failed"))
            except ValueError, KeyError:
                messages.append(label + ": " + (result.stderr.strip() or result.stdout.strip()))
        elif result.returncode:
            messages.append(label + ":\n" + (result.stdout.strip() or result.stderr.strip()))
    CACHE.mkdir(parents=True, exist_ok=True)
    cache.write_text(json.dumps(current), encoding="utf-8")
    if messages:
        feedback("\n\n".join(messages))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        feedback(f"Python diagnostics unavailable: {error}")
