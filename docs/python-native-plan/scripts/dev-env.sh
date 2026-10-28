#!/usr/bin/env bash
# 在工程终端 source；缓存与实例不进入源码目录。
NOUS_REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
export NOUS_REPO_ROOT
export UV_CACHE_DIR="$NOUS_REPO_ROOT/temp/cache/uv"
export CARGO_TARGET_DIR="$NOUS_REPO_ROOT/temp/build/native"
export PYTHONPYCACHEPREFIX="$NOUS_REPO_ROOT/temp/cache/pycache"
mkdir -p "$UV_CACHE_DIR" "$CARGO_TARGET_DIR" "$PYTHONPYCACHEPREFIX"
