[CmdletBinding()]
param(
    [string]$Destination = (Join-Path $env:USERPROFILE ".local\bin"),
    [string]$Source = (Join-Path $PSScriptRoot "..\target\release\rtk-wad.exe"),
    [ValidateSet("rtk-wad", "rtk-wsl", "rtk-wsl1")]
    [string]$CommandName = "rtk-wad",
    [string]$TokenizerRoot,
    [string]$TokenizerPython,
    [switch]$InstallPython,
    [switch]$ConfirmPythonInstall,
    [switch]$SkipTokenizer,
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$source = (Resolve-Path -LiteralPath $source).Path
$targetDirectory = [System.IO.Path]::GetFullPath($Destination)
$target = Join-Path $targetDirectory "$CommandName.exe"
$temporary = Join-Path $targetDirectory ".$CommandName.exe.$PID.new"
$tokenizerInstaller = Join-Path $PSScriptRoot "install-tokenizer.ps1"

New-Item -ItemType Directory -Path $targetDirectory -Force | Out-Null
if ($CommandName -eq "rtk-wad" -and -not $SkipTokenizer) {
    $tokenizerArguments = @{
        Python = $TokenizerPython
        InstallPython = $InstallPython
        ConfirmPythonInstall = $ConfirmPythonInstall
    }
    if ($TokenizerRoot) { $tokenizerArguments.Root = $TokenizerRoot }
    & $tokenizerInstaller @tokenizerArguments
    if ($LASTEXITCODE -ne 0) { throw "WAD tokenizer dependency installation failed." }
}
try {
    Copy-Item -LiteralPath $source -Destination $temporary -ErrorAction Stop

    if (Test-Path -LiteralPath $target) {
        if (-not $Force) {
            throw "Refusing to overwrite existing $target. Re-run with -Force after reviewing it."
        }
        $backup = "$target.previous.exe"
        if (Test-Path -LiteralPath $backup) {
            throw "Refusing to overwrite existing backup $backup. Move or remove it deliberately first."
        }
        Move-Item -LiteralPath $target -Destination $backup
    }

    Move-Item -LiteralPath $temporary -Destination $target
} finally {
    if (Test-Path -LiteralPath $temporary) {
        Remove-Item -LiteralPath $temporary -Force
    }
}

Write-Output "Installed $target"
