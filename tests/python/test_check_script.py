"""Run the development check against a disposable checkout."""

import shutil
import subprocess
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "check"
PY_SOURCE = '''"""module doc
"""

# only comment

def f():
    """func doc"""
    value = """
alpha
beta
"""
    return value
'''
RS_SOURCE = """//! crate

#[doc = "note"]
fn main() {
    // line
    let text = "keep // this
and this";
    let raw = r#"
inside
"#;
    /* block
    still */
    let _value = 1;
}
"""
RECORD = """\
import sys
from pathlib import Path
marker = Path('markers')
marker.mkdir(exist_ok=True)
(marker / sys.argv[1]).write_text('\\n'.join(sys.argv[1:]), encoding='utf-8')
"""
FORMAT_PY = f"""\
import sys
data = sys.stdin.read()
formatted = {PY_SOURCE!r}
if "HUGE" in data:
    sys.stdout.write("x = 1\\n" * 1000)
elif "THREE" in data:
    sys.stdout.write("x = 1\\n" * 3)
elif "TWO" in data:
    sys.stdout.write("x = 1\\n" * 2)
elif "DOC" in data:
    sys.stdout.write(formatted)
else:
    sys.stdout.write("x = 1\\n")
"""
FORMAT_RS = f"""\
import sys
from pathlib import Path
path = Path(sys.argv[-1])
text = path.read_text(encoding="utf-8")
Path("markers").mkdir(exist_ok=True)
Path("markers/rs-length").write_text("ran", encoding="utf-8")
sys.stdout.write({RS_SOURCE!r} if "RUST" in text else text)
"""


def quote(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def command(*args: str) -> str:
    return "[" + ", ".join(quote(arg) for arg in args) + "]"


@pytest.fixture
def checkout(tmp_path: Path) -> Path:
    repo = tmp_path / "checkout"
    shutil.copytree(SCRIPT, repo / "scripts" / "check")
    tools = repo / "tools"
    tools.mkdir()
    (tools / "record.py").write_text(RECORD, encoding="utf-8")
    (tools / "format_py.py").write_text(FORMAT_PY, encoding="utf-8")
    (tools / "format_rs.py").write_text(FORMAT_RS, encoding="utf-8")
    return repo


def write_config(repo: Path, *, warn: int = 800, reject: int = 900) -> None:
    python = Path(sys.executable).as_posix()
    tools = repo / "tools"
    record = (tools / "record.py").as_posix()
    format_py = (tools / "format_py.py").as_posix()
    format_rs = (tools / "format_rs.py").as_posix()
    text = f"""
exclude_roots = ["legacy", "temp"]
exclude_dirs = ["target", "__pycache__", ".venv"]

[length.python]
warn = {warn}
reject = {reject}
suffix = ".py"
roots = ["src", "tests/python", "scripts"]
input = "stdin"
format = {command(python, "-B", format_py, "{path}")}

[length.rust]
warn = 500
reject = 600
suffix = ".rs"
roots = ["native"]
input = "path"
format = {command(python, "-B", format_rs, "{path}")}

[format.python]
argv = {command(python, "-B", record, "py-format")}
paths = ["src", "tests/python", "scripts"]

[format.rust]
argv = {command(python, "-B", record, "rs-format")}
subset = {command(python, "-B", record, "rs-subset")}

[lint.python]
argv = {command(python, "-B", record, "lint")}
paths = ["src"]

[types.python]
argv = {command(python, "-B", record, "types")}

[tests.python]
argv = {command(python, "-B", record, "pytest")}
full = ["tests/python/FULL"]

[[tests.rust]]
package = "nous-vcp"
argv = {command(python, "-B", record, "cargo-vcp")}

[[tests.rust]]
package = "nous-retrieval"
argv = {command(python, "-B", record, "cargo-retrieval")}

[[groups]]
id = "fast"
steps = ["length", "format"]

[[groups]]
id = "static"
steps = ["lint", "types"]

[[groups]]
id = "tests"
steps = ["tests"]

[profiles.default]
groups = ["fast", "static"]
languages = ["python", "rust"]
empty = "tree"

[profiles.ci]
groups = ["fast", "static", "tests"]
languages = ["python"]
empty = "tree"

[profiles.hook]
groups = ["fast"]
languages = ["python", "rust"]
empty = "pass"
"""
    (repo / "check.toml").write_text(text, encoding="utf-8")


def invoke(
    repo: Path,
    *args: str,
    stdin: bytes | None = None,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        [
            sys.executable,
            "-B",
            str(repo / "scripts/check/manage.py"),
            "--config",
            "check.toml",
            *args,
        ],
        cwd=repo,
        input=stdin,
        capture_output=True,
        timeout=30,
    )
    return subprocess.CompletedProcess(
        result.args,
        result.returncode,
        result.stdout.decode("utf-8", "replace"),
        result.stderr.decode("utf-8", "replace"),
    )


def marker(repo: Path, name: str) -> str | None:
    path = repo / "markers" / name
    return path.read_text(encoding="utf-8") if path.exists() else None


def test_counts_formatted_code_without_writing_sources(checkout: Path) -> None:
    write_config(checkout)
    python = checkout / "src" / "sample.py"
    rust = checkout / "native" / "sample.rs"
    python.parent.mkdir(parents=True)
    rust.parent.mkdir(parents=True)
    python.write_text("DOC\nx = 1\n", encoding="utf-8")
    rust.write_text("RUST\nfn x(\n", encoding="utf-8")
    before = python.read_bytes(), rust.read_bytes()
    py = invoke(checkout, "length", "src/sample.py")
    rs = invoke(checkout, "length", "native/sample.rs")
    assert py.returncode == 0, py.stderr + py.stdout
    assert rs.returncode == 0, rs.stderr + rs.stdout
    assert "src/sample.py  6  通过" in py.stdout
    assert "native/sample.rs  9  通过" in rs.stdout
    assert (python.read_bytes(), rust.read_bytes()) == before
    assert "rs-length" not in py.stdout


def test_reject_skips_later_group_and_warning_continues(checkout: Path) -> None:
    write_config(checkout, warn=1, reject=2)
    source = checkout / "src" / "sample.py"
    source.parent.mkdir()
    source.write_text("THREE\n", encoding="utf-8")
    rejected = invoke(checkout)
    assert rejected.returncode == 1, rejected.stdout
    assert "src/sample.py  3 > 2" in rejected.stdout
    assert marker(checkout, "py-format") is not None
    assert marker(checkout, "lint") is None
    assert marker(checkout, "types") is None
    assert "跳过" in rejected.stdout
    shutil.rmtree(checkout / "markers")
    source.write_text("TWO\n", encoding="utf-8")
    warned = invoke(checkout)
    assert warned.returncode == 0, warned.stderr + warned.stdout
    assert "通过，警告 1" in warned.stdout
    assert marker(checkout, "lint") is not None
    assert marker(checkout, "types") is not None


def test_single_file_stdin_and_non_source_paths(checkout: Path) -> None:
    write_config(checkout)
    small = checkout / "src" / "small.py"
    huge = checkout / "src" / "huge.py"
    small.parent.mkdir()
    small.write_text("x = 1\n", encoding="utf-8")
    huge.write_text("HUGE\n", encoding="utf-8")
    (checkout / "notes.md").write_text("note", encoding="utf-8")
    one = invoke(checkout, "length", "src/small.py")
    assert one.returncode == 0, one.stdout
    assert "src/small.py  1  通过" in one.stdout
    assert "huge.py" not in one.stdout
    mixed = invoke(
        checkout,
        "--profile",
        "hook",
        "--stdin",
        "--zero",
        stdin=b"src/small.py\0notes.md\0",
    )
    assert mixed.returncode == 0, mixed.stderr + mixed.stdout
    assert "src/small.py" in mixed.stdout
    assert marker(checkout, "py-format") is not None
    assert "src/small.py" in (marker(checkout, "py-format") or "")
    assert marker(checkout, "rs-format") is None
    assert marker(checkout, "rs-subset") is None
    assert marker(checkout, "rs-length") is None
    skipped = invoke(checkout, "--profile", "hook", "length", "--stdin", stdin=b"notes.md\n")
    assert skipped.returncode == 0, skipped.stderr
    named = invoke(checkout, "length", "notes.md")
    assert named.returncode == 2
    assert "没有可检查的源码文件" in named.stderr


def test_partial_tests_and_ci_profile(checkout: Path) -> None:
    write_config(checkout)
    source = checkout / "src" / "sample.py"
    source.parent.mkdir()
    source.write_text("x = 1\n", encoding="utf-8")
    (checkout / "native").mkdir()
    (checkout / "native" / "lib.rs").write_text("fn main() {}\n", encoding="utf-8")
    missing = invoke(checkout, "tests")
    assert missing.returncode == 2
    assert marker(checkout, "pytest") is None
    partial = invoke(checkout, "tests", "--", "tests/python/test_cli.py", "-k", "name")
    assert partial.returncode == 0, partial.stderr + partial.stdout
    recorded = marker(checkout, "pytest") or ""
    assert "tests/python/test_cli.py" in recorded
    assert "-k" in recorded and "name" in recorded
    assert "FULL" not in recorded
    shutil.rmtree(checkout / "markers")
    rust = invoke(checkout, "tests", "--rust", "nous-vcp")
    assert rust.returncode == 0, rust.stderr + rust.stdout
    assert marker(checkout, "cargo-vcp") is not None
    assert marker(checkout, "pytest") is None
    assert marker(checkout, "cargo-retrieval") is None
    shutil.rmtree(checkout / "markers")
    ci = invoke(checkout, "--profile", "ci")
    assert ci.returncode == 0, ci.stderr + ci.stdout
    assert marker(checkout, "py-format") is not None
    assert marker(checkout, "pytest") is not None
    assert marker(checkout, "rs-format") is None
    assert marker(checkout, "rs-subset") is None
    assert marker(checkout, "rs-length") is None
    assert marker(checkout, "cargo-vcp") is None
    assert marker(checkout, "cargo-retrieval") is None
