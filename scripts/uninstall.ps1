[CmdletBinding()]
param(
    [string]$Destination = (Join-Path $env:USERPROFILE ".local\bin"),
    [switch]$RestorePrevious
)

$ErrorActionPreference = "Stop"
$targetDirectory = [System.IO.Path]::GetFullPath($Destination)
$target = Join-Path $targetDirectory "xuva.exe"
$legacyTarget = Join-Path $targetDirectory "rtk-wad.exe"
$backup = "$target.previous.exe"
$legacyBackup = "$legacyTarget.previous.exe"

if (-not (Test-Path -LiteralPath $target) -and -not (Test-Path -LiteralPath $legacyTarget)) {
    throw "No installed XUVA or legacy RTK-WAD launcher found in $targetDirectory."
}

if ($RestorePrevious) {
    foreach ($entry in @(@($target, $backup), @($legacyTarget, $legacyBackup))) {
        $active, $previous = $entry
        if ((Test-Path -LiteralPath $active) -and (Test-Path -LiteralPath $previous)) {
            Remove-Item -LiteralPath $active
            Move-Item -LiteralPath $previous -Destination $active
        }
    }
    Write-Output "Restored available XUVA and RTK-WAD launcher backups."
} else {
    foreach ($active in @($target, $legacyTarget)) {
        if (Test-Path -LiteralPath $active) { Remove-Item -LiteralPath $active }
    }
    Write-Output "Removed XUVA and legacy RTK-WAD launchers."
}
