"""Exercise the cleanup command against disposable checkouts, never real temp data."""

import os
import shutil
import signal
import subprocess
import sys
from collections.abc import Mapping
from pathlib import Path
from typing import TYPE_CHECKING

import pytest

if TYPE_CHECKING:
    import fcntl
else:
    fcntl = pytest.importorskip("fcntl")
pytestmark = pytest.mark.skipif(sys.platform != "linux", reason="Linux /proc cleanup contract")
SCRIPT = Path(__file__).resolve().parents[3] / "scripts" / "temp"


@pytest.fixture
def checkout(tmp_path: Path) -> Path:
    repo = tmp_path / "checkout with spaces"
    (repo / "scripts").mkdir(parents=True)
    shutil.copytree(SCRIPT, repo / "scripts/temp")
    (repo / "temp").mkdir()
    return repo


def invoke(
    repo: Path, *args: str, cwd: Path | None = None, env: Mapping[str, str] | None = None
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["sh", str(repo / "scripts/temp/run.sh"), *args],
        cwd=cwd or repo,
        env=env,
        text=True,
        capture_output=True,
        timeout=30,
    )


def write(repo: Path, path: str, value: str = "fixture") -> Path:
    target = repo / "temp" / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(value)
    return target


def test_init_creates_agents_without_overwriting_existing_file(checkout: Path) -> None:
    agents = checkout / "temp/AGENTS.md"
    agents.write_text("User instructions", encoding="utf-8")
    result = invoke(checkout, "init")
    assert result.returncode == 0, result.stderr
    assert agents.read_text(encoding="utf-8") == "User instructions"


def test_preview_and_apply_preserve_samples_and_toolchains(checkout: Path) -> None:
    sample = write(checkout, "loose samples/图片 'line\n2'.png", "sample bytes")
    write(checkout, "loose samples/transcript.txt", "words")
    write(checkout, "loose samples/CUSTOM-ARRAY-SCHEMA.JSON", "schema bytes")
    write(checkout, "work/job/asset.png", "disposable build output")
    tool = write(checkout, "tools/ffmpeg", "tool")
    venv = write(checkout, "venvs/python", "interpreter")
    existing = write(checkout, "test-materials/Text/keep.md", "keep")
    agents = write(checkout, "AGENTS.md", "local instructions")
    before = sorted(str(path.relative_to(checkout)) for path in checkout.rglob("*"))
    for args in ((), ("status",), ("clean",)):
        result = invoke(checkout, *args)
        assert result.returncode == 0, result.stderr
        assert sorted(str(path.relative_to(checkout)) for path in checkout.rglob("*")) == before
    result = invoke(checkout, "clean", "--apply")
    assert result.returncode == 0, result.stderr
    assert not sample.exists()
    assert (
        checkout / "temp/test-materials/Incoming/loose samples" / sample.name
    ).read_text() == "sample bytes"
    assert (
        checkout / "temp/test-materials/Incoming/loose samples/transcript.txt"
    ).read_text() == "words"
    assert (
        checkout / "temp/test-materials/Incoming/loose samples/CUSTOM-ARRAY-SCHEMA.JSON"
    ).read_text() == "schema bytes"
    assert not (checkout / "temp/test-materials/Incoming/work").exists()
    assert not (checkout / "temp/work/job").exists()
    assert tool.exists() and venv.exists() and existing.exists()
    assert agents.read_text() == "local instructions"
    # The PowerShell-style switches are accepted too.
    result = invoke(checkout, "clean", "-Apply", "-IncludeToolchains")
    assert result.returncode == 0, result.stderr
    assert not tool.exists() and not venv.exists()
    assert (checkout / "temp/tools").is_dir()


def test_preservation_conflict_prevents_every_deletion(checkout: Path) -> None:
    sample = write(checkout, "loose/sample.pdf", "source")
    destination = write(checkout, "test-materials/Incoming/loose/sample.pdf", "different")
    garbage = write(checkout, "work/job/file", "disposable")
    result = invoke(checkout, "clean", "--apply")
    assert result.returncode != 0
    assert sample.read_text() == "source"
    assert destination.read_text() == "different"
    assert garbage.exists()


def test_nested_worktree_retains_ancestor(checkout: Path, tmp_path: Path) -> None:
    source = tmp_path / "source"
    source.mkdir()
    subprocess.run(["git", "init", "-q", str(source)], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(source),
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.test",
            "commit",
            "--allow-empty",
            "-qm",
            "fixture",
        ],
        check=True,
    )
    nested = checkout / "temp/work/task/nested/tree"
    nested.parent.mkdir(parents=True)
    subprocess.run(
        ["git", "-C", str(source), "worktree", "add", "--detach", str(nested)],
        check=True,
        capture_output=True,
    )
    sibling = write(checkout, "work/task/keep")
    garbage = write(checkout, "cache/remove")
    try:
        result = invoke(checkout, "clean", "--apply")
        assert result.returncode == 0, result.stderr
        assert "linked worktree" in result.stdout
        assert (nested / ".git").exists() and sibling.exists()
        assert not garbage.exists()
    finally:
        subprocess.run(
            ["git", "-C", str(source), "worktree", "remove", "--force", str(nested)], check=True
        )


@pytest.mark.parametrize(
    "reference", ["cwd", "descriptor", "relative_argument", "newline_argument", "symlink_argument"]
)
def test_linux_live_process_references_retain_candidate(checkout: Path, reference: str) -> None:
    name = "line\nbreak.txt" if reference == "newline_argument" else "held.txt"
    held = write(checkout, "work/job/" + name)
    command = [sys.executable, "-c", "import time; print('ready', flush=True); time.sleep(60)"]
    cwd = held.parent if reference == "cwd" else checkout
    if reference == "descriptor":
        # The file path arrives over stdin, not in argv; use a high descriptor.
        command[2] = (
            "import os,sys,time; "
            "fds=[open(os.devnull) for _ in range(80)]; "
            "held=open(sys.stdin.readline().rstrip('\\n')); "
            "print('ready', flush=True); time.sleep(60)"
        )
    if reference in ("relative_argument", "newline_argument"):
        command.append("--input=temp/work/job/" + name)
    if reference == "symlink_argument":
        (checkout / "input.txt").symlink_to(held)
        command.append("--input=input.txt")
    child = subprocess.Popen(
        command, cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True
    )
    assert child.stdin is not None and child.stdout is not None
    try:
        if reference == "descriptor":
            child.stdin.write(str(held) + "\n")
            child.stdin.flush()
        assert child.stdout.readline().strip() == "ready"
        result = invoke(checkout, "clean", "--apply")
        assert result.returncode == 0, result.stderr
        assert held.exists()
        assert f"进程 {child.pid}" in result.stdout
    finally:
        child.terminate()
        child.wait(timeout=10)
        child.stdin.close()
        child.stdout.close()
    result = invoke(checkout, "clean", "--apply")
    assert result.returncode == 0, result.stderr
    assert not held.exists()


def test_links_never_escape_and_init_rejects_layout_conflicts(
    checkout: Path, tmp_path: Path
) -> None:
    outside = tmp_path / "outside"
    outside.mkdir()
    precious = outside / "precious.png"
    precious.write_text("keep")
    (checkout / "temp/linked").symlink_to(outside, target_is_directory=True)
    nested = write(checkout, "work/job/file")
    (nested.parent / "outside").symlink_to(outside, target_is_directory=True)
    result = invoke(checkout, "clean", "--apply")
    assert result.returncode == 0, result.stderr
    assert precious.read_text() == "keep"
    assert (checkout / "temp/linked").is_symlink()
    assert not nested.exists()
    # A protected destination symlink must stop preservation before any deletion.
    (checkout / "temp/test-materials/Incoming").rmdir()
    (checkout / "temp/test-materials/Incoming").symlink_to(outside, target_is_directory=True)
    sample = write(checkout, "samples/new.png")
    result = invoke(checkout, "clean", "--apply")
    assert result.returncode != 0
    assert sample.exists() and not (outside / "samples/new.png").exists()
    result = invoke(checkout, "init")
    assert result.returncode != 0
    assert precious.read_text() == "keep"


def fake_docker(repo: Path, tmp_path: Path, body: str) -> dict[str, str]:
    binary = tmp_path / "bin"
    binary.mkdir()
    docker = binary / "docker"
    docker.write_text("#!/bin/sh\n" + body)
    docker.chmod(0o755)
    return dict(os.environ, PATH=str(binary) + os.pathsep + os.environ["PATH"], CHECKOUT=str(repo))


def test_mount_inspection_failure_and_bind_ancestor_are_safe(
    checkout: Path, tmp_path: Path
) -> None:
    held = write(checkout, "work/job/file")
    env = fake_docker(checkout, tmp_path, "exit 1\n")
    result = invoke(checkout, "clean", "--apply", env=env)
    assert result.returncode != 0 and held.exists()
    docker = Path(env["PATH"].split(os.pathsep)[0]) / "docker"
    docker.write_text(
        '#!/bin/sh\nif [ "$1" = ps ]; then echo fixture; else\n'
        'printf \'[{"Mounts":[{"Type":"bind","Source":"%s/temp"}]}]\\n\' "$CHECKOUT"\nfi\n'
    )
    result = invoke(checkout, "clean", "--apply", env=env)
    assert result.returncode == 0, result.stderr
    assert held.exists() and "bind mount" in result.stdout


def test_concurrent_cleanup_is_refused_and_late_changes_are_retained(
    checkout: Path, tmp_path: Path
) -> None:
    sample = write(checkout, "samples/original.pdf", "original")
    with (checkout / "temp/.cleanup.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        result = invoke(checkout, "clean", "--apply")
        assert result.returncode != 0 and sample.exists()
        assert "另一个 temp 清理" in result.stderr
    # Docker is inspected again immediately before removal. Change the tree then.
    env = fake_docker(
        checkout,
        tmp_path,
        'if [ -e "$CHECKOUT/temp/test-materials/Incoming/samples/original.pdf" ]; then\n'
        'printf new > "$CHECKOUT/temp/samples/new.png"\nfi\n',
    )
    result = invoke(checkout, "clean", "--apply", env=env)
    assert result.returncode != 0
    assert sample.read_text() == "original"
    assert (sample.parent / "new.png").read_text() == "new"
    assert "发生变化" in result.stderr


def test_unreadable_current_user_process_blocks_apply(checkout: Path) -> None:
    held = write(checkout, "work/job/file")
    child = subprocess.Popen(
        [
            sys.executable,
            "-c",
            "import ctypes,time; "
            "assert ctypes.CDLL(None).prctl(4, 0, 0, 0, 0) == 0; "
            "print('ready', flush=True); time.sleep(60)",
        ],
        cwd=checkout,
        stdout=subprocess.PIPE,
        text=True,
    )
    assert child.stdout is not None
    try:
        assert child.stdout.readline().strip() == "ready"
        # PR_SET_DUMPABLE=0 denies same-user /proc cwd/exe/fd reads on Linux.
        try:
            os.readlink(f"/proc/{child.pid}/cwd")
        except PermissionError:
            pass
        else:
            pytest.skip("This runner has permission to inspect nondumpable processes")
        result = invoke(checkout, "clean", "--apply")
        assert result.returncode != 0
        assert held.exists() and "无法完整检查当前用户进程" in result.stderr
    finally:
        child.terminate()
        child.wait(timeout=10)
        child.stdout.close()


def test_hardlinked_cache_files_are_removed_across_candidates(checkout: Path) -> None:
    first = write(checkout, "cache/a", "shared cache bytes")
    same_candidate = first.parent / "b"
    other_candidate = checkout / "temp/build/c"
    other_candidate.parent.mkdir()
    os.link(first, same_candidate)
    os.link(first, other_candidate)
    result = invoke(checkout, "clean", "--apply")
    assert result.returncode == 0, result.stderr
    assert not first.exists() and not same_candidate.exists() and not other_candidate.exists()


def test_uv_protects_only_its_own_cache(checkout: Path) -> None:
    uv = shutil.which("uv")
    if uv is None:
        pytest.skip("uv is unavailable")
    cache = checkout / "temp/cache/uv"
    cache.mkdir(parents=True)
    (cache / "valuable").write_text("active")
    env = {**os.environ, "UV_CACHE_DIR": str(cache)}
    child = subprocess.Popen(
        [
            uv,
            "run",
            "--no-project",
            "--offline",
            sys.executable,
            "-c",
            "import time; print('ready', flush=True); time.sleep(60)",
        ],
        cwd=checkout,
        env=env,
        stdout=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    assert child.stdout is not None
    try:
        assert child.stdout.readline().strip() == "ready"
        result = invoke(checkout, "clean", "cache", "--apply")
        assert result.returncode != 0 and cache.exists()
        assert f"进程 {child.pid}" in result.stdout
    finally:
        os.killpg(child.pid, signal.SIGTERM)
        child.wait(timeout=10)
        child.stdout.close()
    result = invoke(checkout, "clean", "cache", "--apply")
    assert result.returncode == 0, result.stderr
    assert not cache.exists()
