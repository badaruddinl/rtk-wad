[CmdletBinding()]
param(
    [string]$Destination = (Join-Path $env:USERPROFILE ".local\bin"),
    [string]$Source,
    [string]$TokenizerRoot,
    [string]$TokenizerPython,
    [switch]$InstallTokenizer,
    [switch]$InstallPython,
    [switch]$ConfirmPythonInstall,
    [switch]$SkipTokenizer,
    [switch]$SkipProviderScan,
    [switch]$AddToPath,
    [switch]$Status,
    [switch]$Rollback,
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$targetDirectory = [System.IO.Path]::GetFullPath($Destination)
$target = Join-Path $targetDirectory "xuva.exe"
$backup = "$target.previous.exe"
$temporary = Join-Path $targetDirectory ".xuva.$PID.new.exe"
$tokenizerInstaller = Join-Path $PSScriptRoot "install-tokenizer.ps1"

function Get-NormalizedFullPath([string]$Value) {
    if (-not $Value) { return $null }
    try {
        return [System.IO.Path]::GetFullPath($Value).TrimEnd("\")
    } catch {
        return $null
    }
}

function Test-DirectoryOnUserPath([string]$Directory) {
    $normalized = Get-NormalizedFullPath -Value $Directory
    return @(
        [Environment]::GetEnvironmentVariable("Path", "User") -split ";" |
            Where-Object {
                (Get-NormalizedFullPath -Value $_) -eq $normalized
            }
    ).Count -gt 0
}

if ($Status) {
    [pscustomobject]@{
        Installed = Test-Path -LiteralPath $target -PathType Leaf
        Target = $target
        BackupAvailable = Test-Path -LiteralPath $backup -PathType Leaf
        OnUserPath = Test-DirectoryOnUserPath -Directory $targetDirectory
    } | ConvertTo-Json
    return
}

if ($Rollback) {
    if (-not (Test-Path -LiteralPath $backup -PathType Leaf)) {
        throw "No previous XUVA launcher backup found in $targetDirectory."
    }
    $rollbackTemporary = Join-Path $targetDirectory ".xuva.$PID.rollback.exe"
    if (Test-Path -LiteralPath $target) {
        Move-Item -LiteralPath $target -Destination $rollbackTemporary
    }
    try {
        Move-Item -LiteralPath $backup -Destination $target
        if (Test-Path -LiteralPath $rollbackTemporary) {
            Move-Item -LiteralPath $rollbackTemporary -Destination $backup
        }
    } catch {
        if ((-not (Test-Path -LiteralPath $target)) -and
            (Test-Path -LiteralPath $rollbackTemporary)) {
            Move-Item -LiteralPath $rollbackTemporary -Destination $target
        }
        throw
    }
    Write-Output "Restored the previous XUVA launcher."
    return
}

if (-not $Source) {
    $bundledSource = Join-Path $PSScriptRoot "xuva.exe"
    $Source = if (Test-Path -LiteralPath $bundledSource -PathType Leaf) {
        $bundledSource
    } else {
        Join-Path $PSScriptRoot "..\target\release\xuva.exe"
    }
}
$sourcePath = (Resolve-Path -LiteralPath $Source).Path

if ($InstallTokenizer -and $SkipTokenizer) {
    throw "Choose either -InstallTokenizer or the legacy -SkipTokenizer switch, not both."
}
if (-not $InstallTokenizer -and
    ($TokenizerRoot -or $TokenizerPython -or $InstallPython -or $ConfirmPythonInstall)) {
    throw "Tokenizer options require -InstallTokenizer. The core XUVA launcher has no Python or tokenizer dependency."
}
if ($SkipTokenizer) {
    Write-Warning "-SkipTokenizer is no longer needed: tokenizer installation is opt-in."
}

New-Item -ItemType Directory -Path $targetDirectory -Force | Out-Null
if ($InstallTokenizer) {
    $tokenizerArguments = @{
        Python = $TokenizerPython
        InstallPython = $InstallPython
        ConfirmPythonInstall = $ConfirmPythonInstall
    }
    if ($TokenizerRoot) { $tokenizerArguments.Root = $TokenizerRoot }
    & $tokenizerInstaller @tokenizerArguments
    if ($LASTEXITCODE -ne 0) {
        throw "Optional XUVA benchmark tokenizer installation failed."
    }
}

$activated = $false
$hadExisting = Test-Path -LiteralPath $target
try {
    Copy-Item -LiteralPath $sourcePath -Destination $temporary -ErrorAction Stop
    $versionOutput = @(& $temporary --version)
    if (($LASTEXITCODE -ne 0) -or
        (($versionOutput -join "`n") -notmatch '^xuva \d+\.\d+\.\d+')) {
        throw "Candidate launcher failed its local version smoke check."
    }
    if ($hadExisting) {
        if (-not $Force) {
            throw "Refusing to overwrite existing $target. Re-run with -Force after reviewing it."
        }
        if (Test-Path -LiteralPath $backup) {
            Remove-Item -LiteralPath $backup -Force
        }
        Move-Item -LiteralPath $target -Destination $backup
    }

    Move-Item -LiteralPath $temporary -Destination $target
    $activated = $true
    if (-not $SkipProviderScan) {
        & $target scan
        if ($LASTEXITCODE -ne 0) {
            throw "Installed launcher capability scan failed with exit code $LASTEXITCODE."
        }
    }
    foreach ($companion in @("install.ps1", "uninstall.ps1", "xuva-wsl.sh")) {
        $companionSource = Join-Path $PSScriptRoot $companion
        $companionDestination = Join-Path $targetDirectory $companion
        if ((Test-Path -LiteralPath $companionSource -PathType Leaf) -and
            ([System.IO.Path]::GetFullPath($companionSource) -ne
                [System.IO.Path]::GetFullPath($companionDestination))) {
            Copy-Item -LiteralPath $companionSource `
                -Destination $companionDestination -Force
        }
    }
    if ($AddToPath -and -not (Test-DirectoryOnUserPath -Directory $targetDirectory)) {
        $entries = @(
            [Environment]::GetEnvironmentVariable("Path", "User") -split ";" |
                Where-Object { $_ }
        )
        [Environment]::SetEnvironmentVariable(
            "Path",
            ((@($entries) + $targetDirectory) -join ";"),
            "User"
        )
        Write-Output "Added $targetDirectory to the user PATH. Open a new terminal to use it."
    }
} catch {
    if ($activated -and (Test-Path -LiteralPath $target)) {
        Remove-Item -LiteralPath $target -Force
    }
    if ($hadExisting -and (Test-Path -LiteralPath $backup)) {
        Move-Item -LiteralPath $backup -Destination $target
    }
    throw
} finally {
    if (Test-Path -LiteralPath $temporary) {
        Remove-Item -LiteralPath $temporary -Force
    }
}

Write-Output "Installed $target"
