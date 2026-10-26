# Dot-source this file: . ./scripts/dev-env.ps1
$nousRepoRoot = Split-Path -Parent $PSScriptRoot
$nousTempRoot = Join-Path $nousRepoRoot 'temp'
$nousEnvPaths = @{
    TEMP = 'runtime'; TMP = 'runtime'; TMPDIR = 'runtime'
    CARGO_HOME = 'cache/cargo'; CARGO_TARGET_DIR = 'build/cargo'
    UV_CACHE_DIR = 'cache/uv'; PIP_CACHE_DIR = 'cache/pip'
    npm_config_cache = 'cache/npm'; XDG_CACHE_HOME = 'cache/xdg'
    RUFF_CACHE_DIR = 'cache/ruff'; PYTHONPYCACHEPREFIX = 'cache/python-bytecode'
}
foreach ($nousEntry in $nousEnvPaths.GetEnumerator()) {
    $nousPath = Join-Path $nousTempRoot $nousEntry.Value
    New-Item -ItemType Directory -Force -Path $nousPath | Out-Null
    [Environment]::SetEnvironmentVariable($nousEntry.Key, $nousPath, 'Process')
}
Write-Output "Development temporary files: $nousTempRoot"
