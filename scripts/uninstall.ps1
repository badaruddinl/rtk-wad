[CmdletBinding()]
param(
    [string]$Destination = (Join-Path $env:USERPROFILE ".local\bin"),
    [switch]$RestorePrevious
)

$ErrorActionPreference = "Stop"
$targetDirectory = [System.IO.Path]::GetFullPath($Destination)
$target = Join-Path $targetDirectory "xuva.exe"
$backup = "$target.previous.exe"

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
} else {
    Remove-Item -LiteralPath $target
    Write-Output "Removed the XUVA launcher."
}
