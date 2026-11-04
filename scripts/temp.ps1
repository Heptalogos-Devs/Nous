#requires -Version 7.0

<#
.SYNOPSIS
Create, inspect or clean this checkout's standard temp directories.
.DESCRIPTION
init creates missing standard directories without changing existing files. It does
not initialize Nous instances, install tools or change shell environment variables.
status (the default) shows the directory layout and previews cleanup without writes.
clean previews cleanup; clean -Apply collects stray materials and deletes candidates.

Everything in temp is a cleanup candidate except test-materials, tools and venvs.
Old instances, reference repositories, caches and builds are included. PDF, image,
audio, video and associated sample documents are copied to temp/test-materials/Incoming,
preserving their relative paths. Every copy is hash-checked before deletion starts.
Repository/build assets and generated instance storage are not collected as samples.

Close Nous and build commands before applying cleanup. Cleanup recreates the empty
standard directories afterwards, retaining tools and environments unless requested.
Detected running processes, Docker bind mounts and linked Git worktrees are skipped.
The script neither stops services nor removes Docker images or volumes.
.PARAMETER Apply
Execute clean. Only valid with the clean action; otherwise cleanup is read-only.
.PARAMETER Action
init, status or clean. Defaults to status.
.PARAMETER IncludeToolchains
Also remove temp/tools and temp/venvs. This requires reinstalling the local toolchain,
including FFmpeg; the Python environment can be recreated with dev-env.ps1 and uv sync.
.EXAMPLE
pwsh -File ./scripts/temp.ps1 init
.EXAMPLE
pwsh -File ./scripts/temp.ps1 status
.EXAMPLE
pwsh -File ./scripts/temp.ps1 clean -Apply
.EXAMPLE
pwsh -File ./scripts/temp.ps1 clean -Apply -IncludeToolchains
#>

[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet('init', 'status', 'clean')]
    [string]$Action = 'status',
    [switch]$Apply,
    [switch]$IncludeToolchains
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repoRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$tempRoot = Join-Path $repoRoot 'temp'
$materialsRoot = Join-Path $tempRoot 'test-materials'
$pathComparison = if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
if ($Action -ne 'clean' -and ($Apply -or $IncludeToolchains)) {
    throw '-Apply 和 -IncludeToolchains 仅适用于 clean。'
}

# Development environment scripts own individual cache/tool locations within these roots.
$standardRoots = @('build', 'cache', 'runtime', 'work', 'reference-code', 'tools', 'venvs', 'test-materials')
$materialDirectories = @(
    'Audio/Speech/TTS', 'Audio/Speech/FormatChecks', 'Audio/Music',
    'Image/Scenes', 'Image/Text', 'Image/Illustration', 'Image/Pixel',
    'Image/Screenshots', 'Image/Photography', 'Image/Probes',
    'PDF', 'Text', 'Video', 'Instances', 'Config', 'Prompts', 'Schemas',
    'Collections', 'Results/Image', 'Results/Audio/ASR', 'Results/Audio/Music',
    'Exports', 'Diagnostics/Fixtures', 'Incoming'
)

function Test-WithinPath([string]$Path, [string]$Parent) {
    $fullPath = [IO.Path]::GetFullPath($Path)
    $fullParent = [IO.Path]::TrimEndingDirectorySeparator([IO.Path]::GetFullPath($Parent))
    return $fullPath.Equals($fullParent, $pathComparison) -or
        $fullPath.StartsWith($fullParent + [IO.Path]::DirectorySeparatorChar, $pathComparison)
}

function Assert-NoLink([string]$Path) {
    # Check ancestors as well: a lexical path inside temp can otherwise escape via a junction.
    $ancestor = [IO.Path]::GetFullPath($Path)
    while ($ancestor) {
        $item = Get-Item -LiteralPath $ancestor -Force -ErrorAction SilentlyContinue
        if ($item -and ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "拒绝通过符号链接或联接访问：$ancestor"
        }
        $ancestor = [IO.Path]::GetDirectoryName($ancestor)
    }
}

function Assert-TempChild([string]$Path) {
    $fullPath = [IO.Path]::GetFullPath($Path)
    if (-not (Test-WithinPath $fullPath $tempRoot) -or $fullPath.Equals($tempRoot, $pathComparison)) {
        throw "拒绝操作 temp 之外的路径或 temp 本身：$fullPath"
    }
    Assert-NoLink $fullPath
}

function Initialize-TempLayout {
    # Validate the entire layout before creating anything; existing materials stay in place.
    Assert-NoLink $tempRoot
    $directories = @($standardRoots | ForEach-Object { Join-Path $tempRoot $_ }) +
        @($materialDirectories | ForEach-Object { Join-Path $materialsRoot $_ })
    foreach ($directory in $directories) {
        Assert-TempChild $directory
        if ((Test-Path -LiteralPath $directory) -and -not (Test-Path -LiteralPath $directory -PathType Container)) {
            throw "标准目录被同名文件占用：$directory"
        }
    }
    $created = 0
    foreach ($directory in $directories) {
        if (-not (Test-Path -LiteralPath $directory -PathType Container)) {
            [void][IO.Directory]::CreateDirectory($directory)
            $created++
        }
    }
    Write-Host "标准 temp 目录已就绪，新增 $created 个目录。"
}

function Get-BusyPaths {
    # Only paths and process IDs are exposed; command lines may contain credentials.
    if ($IsWindows) {
        $processes = @(Get-CimInstance Win32_Process | Select-Object ProcessId, Name, ExecutablePath, CommandLine)
    } else {
        $processes = @(Get-Process | ForEach-Object {
            [pscustomobject]@{ ProcessId = $_.Id; Name = $_.ProcessName; ExecutablePath = $_.Path; CommandLine = '' }
        })
    }
    $mounts = [Collections.Generic.List[string]]::new()
    if (Get-Command docker -ErrorAction SilentlyContinue) {
        $containerIds = @(& docker ps --quiet 2>$null)
        if ($LASTEXITCODE -ne 0) {
            if ($Apply) { throw '无法检查 Docker 挂载。请启动 Docker 或恢复其连接后重试。' }
            Write-Warning 'Docker 不可用，未检查容器挂载；本次仅预览，实际清理前须恢复检查。'
            $containerIds = @()
        }
        if ($containerIds.Count) {
            $mountLines = @(& docker inspect --format '{{json .Mounts}}' @containerIds 2>$null)
            if ($LASTEXITCODE -ne 0) {
                if ($Apply) { throw '无法读取运行中容器的挂载；未开始清理。' }
                Write-Warning '无法读取容器挂载；本次仅预览，实际清理前须恢复检查。'
                $mountLines = @()
            }
            foreach ($line in $mountLines) {
                foreach ($mount in @($line | ConvertFrom-Json)) {
                    if ($mount.Type -ne 'bind') { continue }
                    $source = [string]$mount.Source
                    if ($IsWindows -and $source -match '^/(?:run/desktop/mnt/host|host_mnt|mnt)/([a-zA-Z])/(.*)$') {
                        $source = $Matches[1] + ':/' + $Matches[2]
                    }
                    if ([IO.Path]::IsPathFullyQualified($source)) {
                        $mounts.Add([IO.Path]::GetFullPath($source))
                    }
                }
            }
        }
    }
    return [pscustomobject]@{ Processes = $processes; Mounts = $mounts }
}

function Get-BlockReason([string]$Path, $Busy) {
    Assert-TempChild $Path
    if (Test-WithinPath $PWD.ProviderPath $Path) { return '当前工作目录位于其中' }
    if (Test-Path -LiteralPath (Join-Path $Path '.git') -PathType Leaf) { return 'Git linked worktree，请用 git worktree remove 处理' }
    foreach ($mount in $Busy.Mounts) {
        if ((Test-WithinPath $mount $Path) -or (Test-WithinPath $Path $mount)) { return '运行中的 Docker bind mount' }
    }
    $absoluteText = $Path.Replace('\', '/')
    $relativeText = [IO.Path]::GetRelativePath($repoRoot, $Path).Replace('\', '/')
    foreach ($process in $Busy.Processes) {
        if ($process.ProcessId -eq $PID) { continue }
        $command = ([string]$process.CommandLine).Replace('\', '/')
        $referencesPath = $command.Contains($absoluteText, $pathComparison) -or $command.Contains($relativeText, $pathComparison)
        $runsHere = $process.ExecutablePath -and (Test-WithinPath $process.ExecutablePath $Path)
        $buildInProgress = ([IO.Path]::GetFileName($Path) -in @('build', 'cache')) -and
            $process.Name -match '^(uv|cargo|rustc|go|bun)(\.exe)?$'
        if ($referencesPath -or $runsHere -or $buildInProgress) { return "进程 $($process.ProcessId) ($($process.Name)) 正在使用" }
    }
    return $null
}

$mediaExtensions = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
foreach ($extension in @(
    '.pdf', '.png', '.jpg', '.jpeg', '.gif', '.webp', '.bmp', '.tif', '.tiff', '.avif', '.heic', '.svg',
    '.wav', '.mp3', '.flac', '.ogg', '.opus', '.m4a', '.aac', '.aiff', '.aif', '.wma', '.amr', '.pcm',
    '.mp4', '.mov', '.mkv', '.webm', '.avi', '.m4v', '.mpg', '.mpeg',
    '.doc', '.docx', '.ppt', '.pptx', '.xls', '.xlsx', '.epub', '.srt', '.vtt'
)) { [void]$mediaExtensions.Add($extension) }

function Test-Material([IO.FileInfo]$File, [string]$TopName) {
    # Do not rescue thousands of icons in downloaded repos or copies in generated storage.
    if ($TopName -match '^(build|cache|dist|runtime|work|reference-code|wheel-installed|sdist-check|tools|venvs|new-api.*|native-.*|nous-.*|pytest-.*)$') { return $false }
    $relative = [IO.Path]::GetRelativePath($tempRoot, $File.FullName)
    if ($relative -match '(^|[\\/])(\.git|node_modules|cache|__pycache__|target|site-packages|content|index|credentials)([\\/]|$)') { return $false }
    if ($mediaExtensions.Contains($File.Extension)) { return $true }
    if ($TopName -in @('media-verification', 'music-samples', 'volcengine-tts', 'volcengine-tts-check')) {
        return $File.Extension -in @('.json', '.txt', '.md', '.html', '.ffconcat', '.ssml', '.bin')
    }
    return $File.Extension -eq '.md' -or $File.Name -eq 'custom-array-schema.json' -or
        ($File.Extension -eq '.txt' -and $File.BaseName -match '(material|sample|transcript|script)')
}

function Copy-VerifiedMaterial([IO.FileInfo]$Source) {
    $destination = Join-Path (Join-Path $materialsRoot 'Incoming') ([IO.Path]::GetRelativePath($tempRoot, $Source.FullName))
    Assert-TempChild $Source.FullName
    Assert-TempChild $destination
    $sourceHash = (Get-FileHash -LiteralPath $Source.FullName -Algorithm SHA256).Hash
    if (Test-Path -LiteralPath $destination) {
        if ((Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash -ne $sourceHash) {
            throw "样本目标已存在且内容不同，保留源文件并停止清理：$destination"
        }
        return
    }
    [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destination))
    $staging = $destination + '.partial-' + [guid]::NewGuid().ToString('N')
    try {
        Copy-Item -LiteralPath $Source.FullName -Destination $staging
        if ((Get-FileHash -LiteralPath $staging -Algorithm SHA256).Hash -ne $sourceHash) {
            throw "样本复制校验失败，未开始删除：$($Source.FullName)"
        }
        Assert-TempChild $destination
        [IO.File]::Move($staging, $destination)
    } finally {
        if (Test-Path -LiteralPath $staging) {
            Assert-TempChild $staging
            Remove-Item -LiteralPath $staging -Force
        }
    }
}

if ($Action -eq 'init') {
    Initialize-TempLayout
    return
}
if (-not (Test-Path -LiteralPath $tempRoot -PathType Container)) {
    Write-Host 'temp/ 不存在。建立目录：pwsh -File ./scripts/temp.ps1 init'
    return
}
Assert-NoLink $tempRoot
Assert-TempChild $materialsRoot
if ($Action -eq 'status') {
    foreach ($name in $standardRoots) {
        $exists = Test-Path -LiteralPath (Join-Path $tempRoot $name) -PathType Container
        Write-Host ('temp/{0}: {1}' -f $name, $(if ($exists) { '已存在' } else { '未建立' }))
    }
}
$busy = Get-BusyPaths
$candidates = [Collections.Generic.List[object]]::new()
$materials = [Collections.Generic.List[IO.FileInfo]]::new()
$retained = [Collections.Generic.List[object]]::new()
$enumeration = [IO.EnumerationOptions]::new()
$enumeration.RecurseSubdirectories = $true
$enumeration.IgnoreInaccessible = $false
$enumeration.AttributesToSkip = [IO.FileAttributes]::ReparsePoint

$mode = if ($Apply) { '归集样本后清理' } else { '只预览' }
Write-Host "扫描 $tempRoot（$mode）..."
foreach ($entry in (Get-ChildItem -LiteralPath $tempRoot -Force | Sort-Object Name)) {
    if ($entry.Name -eq 'test-materials' -or (-not $IncludeToolchains -and $entry.Name -in @('tools', 'venvs'))) {
        $retained.Add([pscustomobject]@{ Path = $entry.Name; Reason = '保留' })
        continue
    }
    if ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        $retained.Add([pscustomobject]@{ Path = $entry.Name; Reason = '符号链接或联接，跳过' })
        continue
    }
    $reason = Get-BlockReason $entry.FullName $busy
    if ($reason) {
        $retained.Add([pscustomobject]@{ Path = $entry.Name; Reason = $reason })
        continue
    }
    [long]$bytes = 0
    $materialCount = 0
    $files = @($entry)
    if ($entry.PSIsContainer) { $files = $entry.EnumerateFiles('*', $enumeration) }
    foreach ($file in $files) {
        $bytes += $file.Length
        if (Test-Material $file $entry.Name) {
            $materials.Add($file)
            $materialCount++
        }
    }
    $candidates.Add([pscustomobject]@{ Path = $entry.FullName; Name = $entry.Name; Bytes = $bytes; Materials = $materialCount })
}

$candidates | Sort-Object Bytes -Descending | Select-Object @{n = '清理路径'; e = { 'temp/' + $_.Name }},
    @{n = 'MiB（文件长度）'; e = { [math]::Round($_.Bytes / 1MB, 1) }}, @{n = '先保留的样本数'; e = { $_.Materials }} | Format-Table -AutoSize
if ($retained.Count) { $retained | Format-Table @{n = '保留路径'; e = { 'temp/' + $_.Path }}, Reason -AutoSize }
[long]$totalBytes = 0
[long]$materialBytes = 0
foreach ($candidate in $candidates) { $totalBytes += $candidate.Bytes }
foreach ($material in $materials) { $materialBytes += $material.Length }
Write-Host ('候选 {0} 项，共 {1:N2} GiB（按文件长度计算，含硬链接）。' -f $candidates.Count, ($totalBytes / 1GB))
Write-Host ('先归集 {0} 份样本及配套资料（{1:N1} MiB）到 temp/test-materials/Incoming/，保留原相对路径。' -f $materials.Count, ($materialBytes / 1MB))
if (-not $Apply) {
    Write-Host '预览完成，未修改文件。执行清理：pwsh -File ./scripts/temp.ps1 clean -Apply'
    return
}

# Finish all preservation first. A failed or conflicting copy prevents every deletion.
foreach ($material in $materials) { Copy-VerifiedMaterial $material }
Write-Host '样本归集及校验完成，开始清理。'
$busy = Get-BusyPaths
$failures = 0
foreach ($candidate in $candidates) {
    try {
        $reason = Get-BlockReason $candidate.Path $busy
        if ($reason) {
            Write-Warning "跳过 $($candidate.Name)：$reason"
            continue
        }
        # PowerShell 7 removes nested links themselves, without traversing their targets.
        Assert-TempChild $candidate.Path
        Remove-Item -LiteralPath $candidate.Path -Recurse -Force
        Write-Host "已清理 temp/$($candidate.Name)"
    } catch {
        $failures++
        Write-Warning "清理失败 $($candidate.Name)：$($_.Exception.Message)"
    }
}
Initialize-TempLayout
if ($failures) { throw "$failures 项未清理成功；解除占用或权限问题后可再次运行。" }
Write-Host '清理完成。测试材料位于 temp/test-materials/。'
