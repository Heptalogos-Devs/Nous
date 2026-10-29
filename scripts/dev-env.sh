#!/usr/bin/env bash
# 在工程终端 source；缓存与实例不进入源码目录。
NOUS_REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
export NOUS_REPO_ROOT
export UV_CACHE_DIR="$NOUS_REPO_ROOT/temp/cache/uv"
export UV_PROJECT_ENVIRONMENT="$NOUS_REPO_ROOT/temp/venvs/python-native"
export UV_PYTHON_INSTALL_DIR="$NOUS_REPO_ROOT/temp/tools/python"
export PIP_CACHE_DIR="$NOUS_REPO_ROOT/temp/cache/pip"
export CARGO_HOME="$NOUS_REPO_ROOT/temp/cache/cargo"
export CARGO_TARGET_DIR="$NOUS_REPO_ROOT/temp/build/native"
export RUFF_CACHE_DIR="$NOUS_REPO_ROOT/temp/cache/ruff"
export PYTHONPYCACHEPREFIX="$NOUS_REPO_ROOT/temp/cache/pycache"
export TEMP="$NOUS_REPO_ROOT/temp/runtime"
export TMP="$TEMP"
export TMPDIR="$TEMP"
export npm_config_cache="$NOUS_REPO_ROOT/temp/cache/npm"
export XDG_CACHE_HOME="$NOUS_REPO_ROOT/temp/cache/xdg"
mkdir -p "$UV_CACHE_DIR" "$CARGO_TARGET_DIR" "$PYTHONPYCACHEPREFIX" \
    "$UV_PYTHON_INSTALL_DIR" "$PIP_CACHE_DIR" "$CARGO_HOME" "$RUFF_CACHE_DIR" \
    "$TEMP" "$npm_config_cache" "$XDG_CACHE_HOME"

export PATH="$UV_PROJECT_ENVIRONMENT/bin:$HOME/.cargo/bin:$PATH"
