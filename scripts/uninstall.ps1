[CmdletBinding()]
param(
    [string]$Destination = (Join-Path $env:USERPROFILE ".local\bin"),
    [switch]$RestorePrevious,
    [switch]$RemoveFromPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$resolvedDestination = [System.IO.Path]::GetFullPath($Destination)
$filesystemRoot = [System.IO.Path]::GetPathRoot($resolvedDestination)
if ($resolvedDestination.TrimEnd("\") -eq $filesystemRoot.TrimEnd("\")) {
    throw "Destination must be a dedicated XUVA bundle directory, not a filesystem root."
}
$targetDirectory = $resolvedDestination.TrimEnd("\")
$parentDirectory = Split-Path -Parent $targetDirectory
$bundleName = Split-Path -Leaf $targetDirectory
$target = Join-Path $targetDirectory "xuva.exe"
$previousDirectory = Join-Path $parentDirectory "$bundleName.previous"
$nonce = "$PID-$([guid]::NewGuid().ToString('N'))"
$removedDirectory = Join-Path $parentDirectory ".$bundleName.removed-$nonce"
$removedPreviousDirectory = Join-Path $parentDirectory ".$bundleName.previous-removed-$nonce"

function Get-NormalizedFullPath([string]$Value) {
    if (-not $Value) { return $null }
    try {
        return [System.IO.Path]::GetFullPath($Value).TrimEnd("\")
    } catch {
        return $null
    }
}

function Invoke-TestFailure([string]$Point) {
    if ($env:XUVA_TEST_MODE -eq "1" -and $env:XUVA_TEST_UNINSTALL_FAILURE -eq $Point) {
        throw "Injected XUVA uninstaller failure at $Point."
    }
}

if (-not (Test-Path -LiteralPath $target -PathType Leaf)) {
    throw "No installed XUVA bundle found in $targetDirectory."
}

if ($RestorePrevious) {
    $installer = Join-Path $targetDirectory "install.ps1"
    if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
        throw "Installed XUVA bundle has no rollback helper."
    }
    & $installer -Destination $targetDirectory -Rollback
    return
}

$originalUserPath = [Environment]::GetEnvironmentVariable("Path", "User")
$pathChanged = $false
$committed = $false
try {
    if ($RemoveFromPath) {
        $normalized = Get-NormalizedFullPath -Value $targetDirectory
        $remaining = @(
            $originalUserPath -split ";" |
                Where-Object {
                    $_ -and (Get-NormalizedFullPath -Value $_) -ne $normalized
                }
        )
        [Environment]::SetEnvironmentVariable("Path", ($remaining -join ";"), "User")
        $pathChanged = $true
    }

    Move-Item -LiteralPath $targetDirectory -Destination $removedDirectory -ErrorAction Stop
    Invoke-TestFailure -Point "after-current-move"
    if (Test-Path -LiteralPath $previousDirectory) {
        Move-Item -LiteralPath $previousDirectory -Destination $removedPreviousDirectory -ErrorAction Stop
    }
    Invoke-TestFailure -Point "after-previous-move"
    $committed = $true
} catch {
    if (-not $committed -and $pathChanged) {
        [Environment]::SetEnvironmentVariable("Path", $originalUserPath, "User")
    }
    if (-not $committed -and
        -not (Test-Path -LiteralPath $previousDirectory) -and
        (Test-Path -LiteralPath $removedPreviousDirectory)) {
        Move-Item -LiteralPath $removedPreviousDirectory -Destination $previousDirectory
    }
    if (-not $committed -and
        -not (Test-Path -LiteralPath $targetDirectory) -and
        (Test-Path -LiteralPath $removedDirectory)) {
        Move-Item -LiteralPath $removedDirectory -Destination $targetDirectory
    }
    throw
}

foreach ($removed in @($removedPreviousDirectory, $removedDirectory)) {
    if (Test-Path -LiteralPath $removed) {
        try {
            Remove-Item -LiteralPath $removed -Recurse -Force -ErrorAction Stop
        } catch {
            Write-Warning "XUVA was uninstalled, but tombstone cleanup must be retried: $removed"
        }
    }
}

Write-Output "Removed the current and previous XUVA bundles."
