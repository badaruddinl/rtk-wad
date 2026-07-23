[CmdletBinding()]
param(
    [string]$Destination = (Join-Path $env:USERPROFILE ".local\bin"),
    [switch]$RestorePrevious
)

$ErrorActionPreference = "Stop"
$targetDirectory = [System.IO.Path]::GetFullPath($Destination)
$target = Join-Path $targetDirectory "rtk-wsl.exe"
$backup = "$target.previous.exe"

if (-not (Test-Path -LiteralPath $target)) {
    throw "No installed launcher found at $target."
}

if ($RestorePrevious) {
    if (-not (Test-Path -LiteralPath $backup)) {
        throw "No previous launcher backup found at $backup."
    }
    Remove-Item -LiteralPath $target
    Move-Item -LiteralPath $backup -Destination $target
    Write-Output "Restored $target from $backup"
} else {
    Remove-Item -LiteralPath $target
    Write-Output "Removed $target. The retained rtk-wsl.cmd wrapper is now the fallback command."
}
