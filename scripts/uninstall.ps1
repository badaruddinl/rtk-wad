[CmdletBinding()]
param(
    [string]$Destination = (Join-Path $env:USERPROFILE ".local\bin"),
    [ValidateSet("rtk-wsl", "rtk-wsl1")]
    [string]$CommandName = "rtk-wsl",
    [switch]$RestorePrevious
)

$ErrorActionPreference = "Stop"
$targetDirectory = [System.IO.Path]::GetFullPath($Destination)
$target = Join-Path $targetDirectory "$CommandName.exe"
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
    if ($CommandName -eq "rtk-wsl") {
        Write-Output "Removed $target. The retained rtk-wsl.cmd wrapper is now the fallback command."
    } else {
        Write-Output "Removed $target."
    }
}
