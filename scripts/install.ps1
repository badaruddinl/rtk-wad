[CmdletBinding()]
param(
    [string]$Destination = (Join-Path $env:USERPROFILE ".local\bin"),
    [string]$Source = (Join-Path $PSScriptRoot "..\target\release\xuva.exe"),
    [string]$TokenizerRoot,
    [string]$TokenizerPython,
    [switch]$InstallTokenizer,
    [switch]$InstallPython,
    [switch]$ConfirmPythonInstall,
    [switch]$SkipTokenizer,
    [switch]$SkipProviderScan,
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$source = (Resolve-Path -LiteralPath $source).Path
$targetDirectory = [System.IO.Path]::GetFullPath($Destination)
$target = Join-Path $targetDirectory "xuva.exe"
$temporary = Join-Path $targetDirectory ".xuva.$PID.new.exe"
$tokenizerInstaller = Join-Path $PSScriptRoot "install-tokenizer.ps1"

if ($InstallTokenizer -and $SkipTokenizer) {
    throw "Choose either -InstallTokenizer or the legacy -SkipTokenizer switch, not both."
}
if (-not $InstallTokenizer -and ($TokenizerRoot -or $TokenizerPython -or $InstallPython -or $ConfirmPythonInstall)) {
    throw "Tokenizer options require -InstallTokenizer. The core WAD launcher has no Python or tokenizer dependency."
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
    if ($LASTEXITCODE -ne 0) { throw "Optional WAD benchmark tokenizer installation failed." }
}
$backup = "$target.previous.exe"
$activated = $false
$hadExisting = Test-Path -LiteralPath $target
try {
    Copy-Item -LiteralPath $source -Destination $temporary -ErrorAction Stop
    $versionOutput = @(& $temporary --version)
    if (($LASTEXITCODE -ne 0) -or (($versionOutput -join "`n") -notmatch '^xuva \d+\.\d+\.\d+')) {
        throw "Candidate launcher failed its local version smoke check."
    }
    if ($hadExisting) {
        if (-not $Force) {
            throw "Refusing to overwrite existing $target. Re-run with -Force after reviewing it."
        }
        if (Test-Path -LiteralPath $backup) {
            throw "Refusing to overwrite existing backup $backup. Move or remove it deliberately first."
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
