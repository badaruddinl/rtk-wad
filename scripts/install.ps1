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
$legacyTarget = Join-Path $targetDirectory "rtk-wad.exe"
$temporary = Join-Path $targetDirectory ".xuva.exe.$PID.new"
$legacyTemporary = Join-Path $targetDirectory ".rtk-wad.exe.$PID.new"
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
try {
    Copy-Item -LiteralPath $source -Destination $temporary -ErrorAction Stop
    Copy-Item -LiteralPath $source -Destination $legacyTemporary -ErrorAction Stop

    foreach ($activeTarget in @($target, $legacyTarget)) {
        if (Test-Path -LiteralPath $activeTarget) {
            if (-not $Force) {
                throw "Refusing to overwrite existing $activeTarget. Re-run with -Force after reviewing it."
            }
            $backup = "$activeTarget.previous.exe"
            if (Test-Path -LiteralPath $backup) {
                throw "Refusing to overwrite existing backup $backup. Move or remove it deliberately first."
            }
            Move-Item -LiteralPath $activeTarget -Destination $backup
        }
    }

    Move-Item -LiteralPath $temporary -Destination $target
    Move-Item -LiteralPath $legacyTemporary -Destination $legacyTarget
} finally {
    foreach ($temporaryTarget in @($temporary, $legacyTemporary)) {
        if (Test-Path -LiteralPath $temporaryTarget) {
            Remove-Item -LiteralPath $temporaryTarget -Force
        }
    }
}

Write-Output "Installed $target and legacy compatibility shim $legacyTarget"
if (-not $SkipProviderScan) {
    & $target scan
    if ($LASTEXITCODE -ne 0) {
        throw "Installed launcher capability scan failed with exit code $LASTEXITCODE."
    }
}
