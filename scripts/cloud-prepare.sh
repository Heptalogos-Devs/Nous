#!/usr/bin/env bash
# Prepare the Python/native developer workspace; existing AGENTS and skills stay untouched.
set -euo pipefail
NOUS_REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$NOUS_REPO_ROOT"
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
export UV_CACHE_DIR="$NOUS_REPO_ROOT/temp/cache/uv"
export CARGO_TARGET_DIR="$NOUS_REPO_ROOT/temp/build/native"
export PYTHONPYCACHEPREFIX="$NOUS_REPO_ROOT/temp/cache/pycache"
mkdir -p "$UV_CACHE_DIR" "$CARGO_TARGET_DIR" "$PYTHONPYCACHEPREFIX"

for NOUS_TOOL in cc c++ pkg-config cmake ffmpeg ffprobe; do
  if ! command -v "$NOUS_TOOL" >/dev/null 2>&1; then
    printf 'Missing system tool: %s\n' "$NOUS_TOOL" >&2
    printf '%s\n' 'Debian/Ubuntu: apt-get install build-essential pkg-config cmake clang lld ca-certificates ffmpeg' >&2
    exit 1
  fi
done
if ! command -v uv >/dev/null 2>&1; then
  curl -LsSf https://astral.sh/uv/install.sh | sh
fi
if ! uv python find 3.14 >/dev/null 2>&1; then
  uv python install 3.14
fi
NOUS_PYTHON="$(uv python find 3.14)"
if ! command -v rustup >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
fi
mapfile -t NOUS_RUST < <("$NOUS_PYTHON" - <<'PY'
from pathlib import Path
import tomllib
toolchain = tomllib.loads(Path('rust-toolchain.toml').read_text())['toolchain']
print(toolchain['channel'])
print(','.join(toolchain.get('components', ['rustfmt', 'clippy'])))
PY
)
rustup toolchain install "${NOUS_RUST[0]}" --profile minimal --component "${NOUS_RUST[1]}"
if [[ ! -f uv.lock ]]; then
  printf '%s\n' 'uv.lock is missing. Resolve and commit the workspace lock before preparing this environment.' >&2
  exit 1
fi
uv sync --locked --all-packages
uv run --no-sync nous --help >/dev/null
printf '%s\n' 'Python, native extension and development tools are ready.'
printf '%s\n' 'Checks: uv run basedpyright; uv run ruff check src sdk/python/src native/python/python'
# This script never fetches governance repositories, replaces skills, runs models, or starts a host.
