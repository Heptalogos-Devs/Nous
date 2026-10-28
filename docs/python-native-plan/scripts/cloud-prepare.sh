#!/usr/bin/env bash
# 仅准备 Linux Codex 环境；不创建分支、不修改源码、不运行测试或模型请求。
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
export UV_CACHE_DIR="$ROOT/temp/cache/uv"
export CARGO_TARGET_DIR="$ROOT/temp/build/native"
export PYTHONPYCACHEPREFIX="$ROOT/temp/cache/pycache"
mkdir -p "$UV_CACHE_DIR" "$CARGO_TARGET_DIR" "$PYTHONPYCACHEPREFIX" "$ROOT/temp/upstream"

if ! command -v uv >/dev/null 2>&1; then
  curl -LsSf https://astral.sh/uv/install.sh | sh
fi
uv python install 3.14
PYTHON="$(uv python find 3.14)"

if ! command -v rustup >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
fi
TOOLCHAIN="$("$PYTHON" - <<'PY'
from pathlib import Path
import tomllib
path = Path('rust-toolchain.toml')
print(tomllib.loads(path.read_text())['toolchain']['channel'] if path.exists() else '1.97.0')
PY
)"
rustup toolchain install "$TOOLCHAIN" --profile minimal --component rustfmt --component clippy

CODEX_DIR="${CODEX_HOME:-$HOME/.codex}"
SKILLS="$HOME/.agents/skills"
if [[ ! -d "$SKILLS" && -d "$CODEX_DIR/skills" ]]; then
  SKILLS="$CODEX_DIR/skills"
fi
if [[ ! -f "$SKILLS/system-architecture/SKILL.md" || ! -f "$SKILLS/persistence-transactions/SKILL.md" || ! -f "$SKILLS/structural-refactoring/SKILL.md" ]] || ! grep -q 'BEGIN_AntiRLHF_Constitution' "$CODEX_DIR/AGENTS.md" 2>/dev/null; then
  UPSTREAM="$ROOT/temp/upstream/AntiRLHF"
  if [[ ! -d "$UPSTREAM/.git" ]]; then
    git clone --depth 1 https://github.com/AntiRLHF-Research/AntiRLHF.git "$UPSTREAM"
  fi
  "$PYTHON" "$UPSTREAM/tools/install.py" --target "$SKILLS" --instruction "$CODEX_DIR/AGENTS.md"
fi

for tool in cc c++ pkg-config ffmpeg ffprobe; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    printf '环境需安装系统工具: %s\n' "$tool" >&2
    printf 'Debian/Ubuntu: apt-get install build-essential pkg-config clang lld cmake ca-certificates ffmpeg\n' >&2
    exit 1
  fi
done

if [[ -f pyproject.toml ]] && "$PYTHON" - <<'PY'
from pathlib import Path
import tomllib
p = tomllib.loads(Path('pyproject.toml').read_text())
raise SystemExit(0 if p.get('project', {}).get('name') == 'nous-wave-server' else 1)
PY
then
  if [[ -f uv.lock ]]; then
    uv sync --locked
  else
    uv lock
    uv sync --locked
  fi
else
  printf '%s\n' '环境工具已准备。Python 主工程建立后执行 uv lock 与 uv sync --locked。'
fi
