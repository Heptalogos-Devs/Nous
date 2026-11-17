#requires -Version 7.0
$python = Get-Command python -CommandType Application -ErrorAction Stop | Select-Object -First 1
& $python.Source -B (Join-Path $PSScriptRoot 'manage.py') @args
exit $LASTEXITCODE
