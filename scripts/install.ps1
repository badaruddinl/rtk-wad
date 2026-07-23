[CmdletBinding()]
param(
    [string]$Destination = (Join-Path $env:USERPROFILE ".local\bin"),
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$source = Join-Path $PSScriptRoot "..\target\release\rtk-wsl.exe"
$source = (Resolve-Path -LiteralPath $source).Path
$targetDirectory = [System.IO.Path]::GetFullPath($Destination)
$target = Join-Path $targetDirectory "rtk-wsl.exe"

New-Item -ItemType Directory -Path $targetDirectory -Force | Out-Null
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

Copy-Item -LiteralPath $source -Destination $target
Write-Output "Installed $target"
