# Dot-source this file: . ./scripts/workspace.ps1 env
param([ValidateSet('env', 'init', 'status')][string]$Action = 'init')
$nousRepoRoot = Split-Path -Parent $PSScriptRoot
$nousTempRoot = Join-Path $nousRepoRoot 'temp'
$nousEnvPaths = @{
    TEMP = 'runtime'; TMP = 'runtime'; TMPDIR = 'runtime'
    UV_CACHE_DIR = 'cache/uv'; PIP_CACHE_DIR = 'cache/pip'
    UV_PROJECT_ENVIRONMENT = 'venvs/nous'; UV_PYTHON_INSTALL_DIR = 'tools/python'
    CARGO_HOME = 'tools/cargo-home'; CARGO_TARGET_DIR = 'build/native'
    npm_config_cache = 'cache/npm'; XDG_CACHE_HOME = 'cache/xdg'
    RUFF_CACHE_DIR = 'cache/ruff'; PYTHONPYCACHEPREFIX = 'cache/python-bytecode'
}
foreach ($nousEntry in $nousEnvPaths.GetEnumerator()) {
    $nousPath = Join-Path $nousTempRoot $nousEntry.Value
    New-Item -ItemType Directory -Force -Path $nousPath | Out-Null
    [Environment]::SetEnvironmentVariable($nousEntry.Key, $nousPath, 'Process')
}
[Environment]::SetEnvironmentVariable('UV_NO_EDITABLE', '1', 'Process')
$nousPythonScripts = Join-Path $env:UV_PROJECT_ENVIRONMENT 'Scripts'
foreach ($nousBin in @($nousPythonScripts, (Join-Path $nousTempRoot 'tools/zvec-grep/node_modules/@zvec/zvec-grep/bin'), (Join-Path $nousTempRoot 'tools/codegraph/node_modules/.bin'))) {
    if (($env:PATH -split [IO.Path]::PathSeparator) -notcontains $nousBin) {
        $env:PATH = $nousBin + [IO.Path]::PathSeparator + $env:PATH
    }
}
$env:ZVEC_GREP_HOME = Join-Path $nousTempRoot 'tools/zvec-grep/runtime'
$env:ZVEC_GREP_MODEL_CACHE = Join-Path $nousTempRoot 'tools/zvec-grep/models'
if ($Action -eq 'env') { return }
$nousUv = Get-Command uv -CommandType Application -ErrorAction Stop | Select-Object -First 1
& $nousUv.Source run --no-project --with tomlkit python -B (Join-Path $PSScriptRoot 'workspace.py') $Action
if ($LASTEXITCODE -ne 0) { throw '工作区初始化失败。' }
