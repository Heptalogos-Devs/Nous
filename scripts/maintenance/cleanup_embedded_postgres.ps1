# Copyright 2026 Aravine Zhu
# SPDX-License-Identifier: Apache-2.0

[CmdletBinding(SupportsShouldProcess)]
param(
    [int]$MinimumAgeHours = 0
)

$ErrorActionPreference = "Stop"
if ($MinimumAgeHours -lt 0) {
    throw "MinimumAgeHours must be non-negative."
}

$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$tempRoot = Join-Path $repoRoot 'data/temp/tests'
if (!(Test-Path -LiteralPath $tempRoot)) { Write-Output 'No local PostgreSQL test roots'; return }
$marker = 'data/PG_VERSION'
$now = Get-Date
$installationRoot = if ($env:NOUS_WAVE_POSTGRES_RUNTIME) {
    [IO.Path]::GetFullPath($env:NOUS_WAVE_POSTGRES_RUNTIME)
} else {
    Join-Path $repoRoot 'data/runtime/installed/postgresql'
}
$pgCtlName = if ($env:OS -eq 'Windows_NT') { 'pg_ctl.exe' } else { 'pg_ctl' }
$pgCtl = Join-Path $installationRoot (Join-Path 'bin' $pgCtlName)

$candidates = Get-ChildItem -LiteralPath $tempRoot -Force -Directory |
    Where-Object {
        $_.Name -like '.tmp*' -and
        (Test-Path -LiteralPath (Join-Path $_.FullName $marker))
    }

$removed = 0
$skippedYoung = 0
$skippedRunning = 0
$unableToDetermine = 0
$failed = 0
foreach ($candidate in $candidates) {
    $resolved = (Resolve-Path -LiteralPath $candidate.FullName).Path
    if (!$resolved.StartsWith($tempRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or ($candidate.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Unexpected cleanup target' }
    $dataDir = Join-Path $candidate.FullName 'data'
    $bytes = 0
    try {
        $bytes = (Get-ChildItem -LiteralPath $candidate.FullName -File -Force -Recurse | Measure-Object -Property Length -Sum).Sum
        if ($null -eq $bytes) { $bytes = 0 }
    } catch {
        Write-Output ("candidate={0} status=unable_to_measure error={1}" -f $candidate.Name, $_.Exception.Message)
    }
    if (($now - $candidate.LastWriteTime).TotalHours -lt $MinimumAgeHours) {
        $skippedYoung++
        Write-Output ("candidate={0} status=skipped_young bytes={1}" -f $candidate.Name, $bytes)
        continue
    }
    if (!(Test-Path -LiteralPath $pgCtl -PathType Leaf)) {
        $unableToDetermine++
        Write-Output ("candidate={0} status=unable_to_determine data_dir={1} reason=pg_ctl_missing path={2}" -f $candidate.Name, $dataDir, $pgCtl)
        continue
    }
    try {
        $arguments = @('status', '-D', $dataDir)
        $statusOutput = @(& $pgCtl @arguments 2>&1)
        $statusCode = $LASTEXITCODE
    } catch {
        $unableToDetermine++
        Write-Output ("candidate={0} status=unable_to_determine data_dir={1} reason={2}" -f $candidate.Name, $dataDir, $_.Exception.Message)
        continue
    }
    if ($statusCode -eq 0) {
        $skippedRunning++
        Write-Output ("candidate={0} status=running bytes={1}" -f $candidate.Name, $bytes)
        continue
    }
    if ($statusCode -ne 3) {
        $unableToDetermine++
        $detail = ($statusOutput -join ' ').Trim()
        Write-Output ("candidate={0} status=unable_to_determine data_dir={1} exit_code={2} detail={3}" -f $candidate.Name, $dataDir, $statusCode, $detail)
        continue
    }
    if ($PSCmdlet.ShouldProcess($candidate.FullName, 'Remove embedded PostgreSQL test root')) {
        try {
            [System.IO.Directory]::Delete($candidate.FullName, $true)
            $removed++
            Write-Output ("candidate={0} status=removed bytes={1}" -f $candidate.Name, $bytes)
        } catch {
            $failed++
            Write-Output ("candidate={0} status=failed bytes={1} error={2}" -f $candidate.Name, $bytes, $_.Exception.Message)
        }
    } else {
        Write-Output ("candidate={0} status=whatif bytes={1}" -f $candidate.Name, $bytes)
    }
}

Write-Output ("embedded_postgres_candidates={0} removed={1} skipped_young={2} skipped_running={3} unable_to_determine={4} failed={5}" -f $candidates.Count, $removed, $skippedYoung, $skippedRunning, $unableToDetermine, $failed)
if ($failed -gt 0 -or $unableToDetermine -gt 0) {
    exit 1
}
