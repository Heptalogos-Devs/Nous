#!/usr/bin/env bash
# 在工程终端 source；缓存与实例不进入源码目录。
NOUS_REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
export NOUS_REPO_ROOT
export UV_CACHE_DIR="$NOUS_REPO_ROOT/temp/cache/uv"
export UV_PROJECT_ENVIRONMENT="$NOUS_REPO_ROOT/temp/venvs/nous"
export UV_NO_EDITABLE=1
export UV_PYTHON_INSTALL_DIR="$NOUS_REPO_ROOT/temp/tools/python"
export CARGO_HOME="$NOUS_REPO_ROOT/temp/tools/cargo-home"
export CARGO_TARGET_DIR="$NOUS_REPO_ROOT/temp/build/native"
export PIP_CACHE_DIR="$NOUS_REPO_ROOT/temp/cache/pip"
export RUFF_CACHE_DIR="$NOUS_REPO_ROOT/temp/cache/ruff"
export PYTHONPYCACHEPREFIX="$NOUS_REPO_ROOT/temp/cache/pycache"
export TEMP="$NOUS_REPO_ROOT/temp/runtime"
export TMP="$TEMP"
export TMPDIR="$TEMP"
export npm_config_cache="$NOUS_REPO_ROOT/temp/cache/npm"
export XDG_CACHE_HOME="$NOUS_REPO_ROOT/temp/cache/xdg"
export ZVEC_GREP_HOME="$NOUS_REPO_ROOT/temp/tools/zvec-grep/runtime"
export ZVEC_GREP_MODEL_CACHE="$NOUS_REPO_ROOT/temp/tools/zvec-grep/models"
mkdir -p "$UV_CACHE_DIR" "$PYTHONPYCACHEPREFIX" \
    "$UV_PYTHON_INSTALL_DIR" "$PIP_CACHE_DIR" "$RUFF_CACHE_DIR" "$CARGO_HOME" "$CARGO_TARGET_DIR" \
    "$TEMP" "$npm_config_cache" "$XDG_CACHE_HOME"

for nous_bin in "$UV_PROJECT_ENVIRONMENT/bin" "$NOUS_REPO_ROOT/temp/tools/codegraph/node_modules/.bin" "$NOUS_REPO_ROOT/temp/tools/zvec-grep/node_modules/@zvec/zvec-grep/bin"; do
    case ":$PATH:" in *":$nous_bin:"*) ;; *) PATH="$nous_bin:$PATH" ;; esac
done
export PATH
if [ "${1:-}" = "env" ]; then
    return 0
fi
uv run --no-project --with tomlkit python -B "$NOUS_REPO_ROOT/scripts/workspace.py" "${1:-init}" || return $?
