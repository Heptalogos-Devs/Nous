"""Exercise explicitly selected temp cleanup against disposable project directories."""

import shutil
import subprocess
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "temp"


@pytest.fixture
def checkout(tmp_path: Path) -> Path:
    repo = tmp_path / "repository with spaces"
    (repo / "scripts").mkdir(parents=True)
    shutil.copytree(SCRIPT, repo / "scripts/temp")
    (repo / "temp").mkdir()
    return repo


def write(repo: Path, name: str, content: str = "data") -> Path:
    path = repo / "temp" / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    return path


def invoke(repo: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(repo / "scripts/temp/manage.py"), *args],
        cwd=repo,
        text=True,
        encoding="utf-8",
        capture_output=True,
        timeout=30,
    )


def test_only_explicit_targets_are_cleaned(checkout: Path) -> None:
    old = write(checkout, "test-materials/Instances/old/data/database", "old instance")
    other = write(checkout, "test-materials/Instances/current/data/database", "keep")
    unrelated = write(checkout, "work/unrelated/file", "keep")
    target = "temp/test-materials/Instances/old"

    preview = invoke(checkout, "clean", target)
    assert preview.returncode == 0, preview.stderr
    assert target in preview.stdout and old.exists() and other.exists()

    result = invoke(checkout, "clean", target, "test-materials/Instances/old/data", "--apply")
    assert result.returncode == 0, result.stderr
    assert not old.exists() and other.read_text(encoding="utf-8") == "keep"
    assert unrelated.exists()
    assert not (checkout / "temp/test-materials/Incoming").exists()


def test_selected_loose_material_is_preserved_before_cleanup(checkout: Path) -> None:
    sample = write(checkout, "loose samples/image.png", "original image")
    unrelated = write(checkout, "loose samples/other.txt", "keep")

    result = invoke(checkout, "clean", "loose samples/image.png", "--apply")
    assert result.returncode == 0, result.stderr
    assert not sample.exists() and unrelated.exists()
    saved = checkout / "temp/test-materials/Incoming/loose samples/image.png"
    assert saved.read_text(encoding="utf-8") == "original image"


def test_selected_cleanup_rejects_outside_paths_and_preserves_user_material(
    checkout: Path, tmp_path: Path
) -> None:
    keep = write(checkout, "test-materials/Text/0-personal.md", "original")
    instruction = write(checkout, "work/task/AGENTS.md", "instruction")
    sibling = write(checkout, "work/task/data", "keep")
    outside = tmp_path / "outside.txt"
    outside.write_text("outside", encoding="utf-8")

    invalid = invoke(checkout, "clean", "work/task", str(outside), "--apply")
    assert invalid.returncode != 0
    assert sibling.exists() and outside.read_text(encoding="utf-8") == "outside"

    for target in ("test-materials/Text", "work/task"):
        result = invoke(checkout, "clean", target, "--apply")
        assert result.returncode != 0
        assert "受保护" in result.stdout
    assert keep.read_text(encoding="utf-8") == "original"
    assert instruction.exists() and sibling.exists()


def test_selected_cleanup_does_not_follow_links(checkout: Path, tmp_path: Path) -> None:
    outside = tmp_path / "outside"
    outside.mkdir()
    retained = outside / "original.txt"
    retained.write_text("keep", encoding="utf-8")
    link = checkout / "temp/linked"
    try:
        link.symlink_to(outside, target_is_directory=True)
    except OSError as error:
        pytest.skip(f"Cannot create directory symlinks on this system: {error}")

    for target in ("linked", "linked/original.txt"):
        result = invoke(checkout, "clean", target, "--apply")
        assert result.returncode != 0
        assert retained.read_text(encoding="utf-8") == "keep"


@pytest.mark.parametrize("prefix", ["", "-I"])
def test_selected_cleanup_preserves_path_used_by_another_process(
    checkout: Path, prefix: str
) -> None:
    held = write(checkout, "work/held/file.txt", "keep")
    child = subprocess.Popen(
        [
            sys.executable,
            "-c",
            "import time; print('ready', flush=True); time.sleep(60)",
            prefix + str(held),
        ],
        cwd=checkout,
        stdout=subprocess.PIPE,
        text=True,
        encoding="utf-8",
    )
    assert child.stdout is not None
    try:
        assert child.stdout.readline().strip() == "ready"
        result = invoke(checkout, "clean", "work/held", "--apply")
        assert result.returncode != 0
        assert f"进程 {child.pid}" in result.stdout and held.exists()
    finally:
        child.terminate()
        child.wait(timeout=10)
        child.stdout.close()


def test_selected_cleanup_does_not_confuse_sibling_prefix(checkout: Path) -> None:
    unused = write(checkout, "cache/unused.txt", "remove")
    sibling = write(checkout, "cache-other/used.txt", "keep")
    child = subprocess.Popen(
        [
            sys.executable,
            "-c",
            "import time; print('ready', flush=True); time.sleep(60)",
            str(sibling),
        ],
        cwd=checkout,
        stdout=subprocess.PIPE,
        text=True,
        encoding="utf-8",
    )
    assert child.stdout is not None
    try:
        assert child.stdout.readline().strip() == "ready"
        result = invoke(checkout, "clean", "cache", "--apply")
        assert result.returncode == 0, result.stderr + result.stdout
        assert not unused.exists() and sibling.exists()
    finally:
        child.terminate()
        child.wait(timeout=10)
        child.stdout.close()
