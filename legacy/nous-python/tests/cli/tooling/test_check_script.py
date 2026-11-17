"""Development checks must not rewrite the files they inspect."""

import shutil
import subprocess
import sys
from pathlib import Path

import tomlkit


def test_check_reads_selected_source_without_changing_it(tmp_path: Path) -> None:
    source_tools = Path(__file__).resolve().parents[3] / "scripts/check"
    checkout = tmp_path / "checkout"
    shutil.copytree(source_tools, checkout / "scripts/check")
    source = checkout / "src/example.py"
    source.parent.mkdir()
    source.write_bytes(b"value=1\n")
    before = source.read_bytes()
    config = tomlkit.parse((source_tools / "config/check.toml").read_text(encoding="utf-8"))
    config["length"]["python"]["format"] = [
        sys.executable,
        "-c",
        'import sys; sys.stdin.read(); print("value = 1")',
    ]
    (checkout / "check.toml").write_text(tomlkit.dumps(config), encoding="utf-8")
    result = subprocess.run(
        [
            sys.executable,
            "scripts/check/manage.py",
            "--config",
            "check.toml",
            "length",
            "src/example.py",
        ],
        cwd=checkout,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=30,
        check=False,
    )
    assert result.returncode == 0, result.stderr + result.stdout
    assert source.read_bytes() == before
