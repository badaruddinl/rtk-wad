[CmdletBinding()]
param(
    [string]$Destination = (Join-Path $env:USERPROFILE ".local\bin"),
    [switch]$RestorePrevious,
    [switch]$RemoveFromPath
)

$ErrorActionPreference = "Stop"
$targetDirectory = [System.IO.Path]::GetFullPath($Destination)
$target = Join-Path $targetDirectory "xuva.exe"
$backup = "$target.previous.exe"

function Get-NormalizedFullPath([string]$Value) {
    if (-not $Value) { return $null }
    try {
        return [System.IO.Path]::GetFullPath($Value).TrimEnd("\")
    } catch {
        return $null
    }
}

if (-not (Test-Path -LiteralPath $target)) {
    throw "No installed XUVA launcher found in $targetDirectory."
}

if ($RestorePrevious) {
    if (-not (Test-Path -LiteralPath $backup)) {
        throw "No previous XUVA launcher backup found in $targetDirectory."
    }
    Remove-Item -LiteralPath $target
    Move-Item -LiteralPath $backup -Destination $target
    Write-Output "Restored the previous XUVA launcher."
    return
}

Remove-Item -LiteralPath $target
if (Test-Path -LiteralPath $backup) {
    Remove-Item -LiteralPath $backup -Force
}
foreach ($companion in @("install.ps1", "uninstall.ps1", "xuva-wsl.sh")) {
    $path = Join-Path $targetDirectory $companion
    if (Test-Path -LiteralPath $path) {
        Remove-Item -LiteralPath $path -Force
    }
}
if ($RemoveFromPath) {
    $normalized = Get-NormalizedFullPath -Value $targetDirectory
    $entries = @(
        [Environment]::GetEnvironmentVariable("Path", "User") -split ";" |
            Where-Object {
                $_ -and ((Get-NormalizedFullPath -Value $_) -ne $normalized)
            }
    )
    [Environment]::SetEnvironmentVariable("Path", ($entries -join ";"), "User")
}
Write-Output "Removed the XUVA launcher."
