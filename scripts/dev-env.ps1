# Dot-source this file: . ./scripts/dev-env.ps1
$nousRepoRoot = Split-Path -Parent $PSScriptRoot
$nousTempRoot = Join-Path $nousRepoRoot 'temp'
$nousEnvPaths = @{
    TEMP = 'runtime'; TMP = 'runtime'; TMPDIR = 'runtime'
    CARGO_HOME = 'cache/cargo'; CARGO_TARGET_DIR = 'build/native'
    UV_CACHE_DIR = 'cache/uv'; PIP_CACHE_DIR = 'cache/pip'
    UV_PROJECT_ENVIRONMENT = 'venvs/python-native'; UV_PYTHON_INSTALL_DIR = 'tools/python'
    npm_config_cache = 'cache/npm'; XDG_CACHE_HOME = 'cache/xdg'
    RUFF_CACHE_DIR = 'cache/ruff'; PYTHONPYCACHEPREFIX = 'cache/python-bytecode'
}
foreach ($nousEntry in $nousEnvPaths.GetEnumerator()) {
    $nousPath = Join-Path $nousTempRoot $nousEntry.Value
    New-Item -ItemType Directory -Force -Path $nousPath | Out-Null
    [Environment]::SetEnvironmentVariable($nousEntry.Key, $nousPath, 'Process')
}
$nousPythonScripts = Join-Path $env:UV_PROJECT_ENVIRONMENT 'Scripts'
if (($env:PATH -split [IO.Path]::PathSeparator) -notcontains $nousPythonScripts) {
    $env:PATH = $nousPythonScripts + [IO.Path]::PathSeparator + $env:PATH
}
Write-Output "Development temporary files: $nousTempRoot"
