[CmdletBinding()]
param(
    [string]$Destination = (Join-Path ([Environment]::GetFolderPath("LocalApplicationData")) "Programs\XUVA"),
    [switch]$RestorePrevious,
    [switch]$RemoveFromPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$lifecycleLibrary = Join-Path $PSScriptRoot "install-lifecycle.ps1"
if (-not (Test-Path -LiteralPath $lifecycleLibrary -PathType Leaf)) {
    throw "XUVA installation lifecycle library is missing."
}
. $lifecycleLibrary

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
$journalPath = Get-XuvaTransactionPath -TargetDirectory $targetDirectory

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

if (Test-Path -LiteralPath $journalPath -PathType Leaf) {
    throw "An interrupted XUVA transaction requires install.ps1 -Recover before uninstall."
}
if (-not (Test-Path -LiteralPath $target -PathType Leaf)) {
    throw "No installed XUVA bundle found in $targetDirectory."
}
$activeMarker = Get-XuvaOwnedBundle -Directory $targetDirectory
if (Test-Path -LiteralPath $previousDirectory) {
    $previousMarker = Get-XuvaOwnedBundle -Directory $previousDirectory
    if ([string]$previousMarker.installation_id -ne [string]$activeMarker.installation_id) {
        throw "Previous bundle does not belong to the active XUVA installation."
    }
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
$uninstallState = @{
    operation = "uninstall"
    phase = "prepared"
    target = $targetDirectory
    previous = $previousDirectory
    stage = $removedDirectory
    auxiliary = $removedPreviousDirectory
    had_existing = $true
    installation_id = [string]$activeMarker.installation_id
}
Write-XuvaTransaction -JournalPath $journalPath -State $uninstallState
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
    $uninstallState.phase = "current_removed"
    Write-XuvaTransaction -JournalPath $journalPath -State $uninstallState
    Invoke-TestFailure -Point "after-current-move"
    if (Test-Path -LiteralPath $previousDirectory) {
        Move-Item -LiteralPath $previousDirectory -Destination $removedPreviousDirectory -ErrorAction Stop
        $uninstallState.phase = "previous_removed"
        Write-XuvaTransaction -JournalPath $journalPath -State $uninstallState
    }
    Invoke-TestFailure -Point "after-previous-move"
    $committed = $true
    $uninstallState.phase = "committed"
    Write-XuvaTransaction -JournalPath $journalPath -State $uninstallState
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
    Remove-Item -LiteralPath $journalPath -Force -ErrorAction SilentlyContinue
    throw
}

$cleanupPending = $false
foreach ($removed in @($removedPreviousDirectory, $removedDirectory)) {
    if (Test-Path -LiteralPath $removed) {
        try {
            Remove-XuvaOwnedDirectory -Directory $removed
        } catch {
            $cleanupPending = $true
            Write-Warning "XUVA was uninstalled, but tombstone cleanup must be retried: $removed"
        }
    }
}
if (-not $cleanupPending) {
    Remove-Item -LiteralPath $journalPath -Force -ErrorAction SilentlyContinue
}

Write-Output "Removed the current and previous XUVA bundles."
