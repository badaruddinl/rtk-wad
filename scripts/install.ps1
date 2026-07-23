[CmdletBinding()]
param(
    [string]$Destination = (Join-Path $env:USERPROFILE ".local\bin"),
    [string]$Source = (Join-Path $PSScriptRoot "..\target\release\rtk-wsl.exe"),
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$source = (Resolve-Path -LiteralPath $source).Path
$targetDirectory = [System.IO.Path]::GetFullPath($Destination)
$target = Join-Path $targetDirectory "rtk-wsl.exe"
$temporary = Join-Path $targetDirectory ".rtk-wsl.exe.$PID.new"

New-Item -ItemType Directory -Path $targetDirectory -Force | Out-Null
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
