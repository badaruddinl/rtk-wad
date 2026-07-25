[CmdletBinding()]
param(
    [string]$RepositoryRoot = (Split-Path -Parent $PSScriptRoot),
    [string]$Source = (Join-Path (Split-Path -Parent $PSScriptRoot) "target\release\rtk-wad.exe")
)

$ErrorActionPreference = "Stop"
$install = Join-Path $RepositoryRoot "scripts\install.ps1"
$uninstall = Join-Path $RepositoryRoot "scripts\uninstall.ps1"
$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) "rtk-wsl-packaging-$PID"
$destination = Join-Path $temporaryRoot "bin"
$target = Join-Path $destination "rtk-wad.exe"
$legacyTarget = Join-Path $destination "rtk-wsl.exe"
$wsl1Target = Join-Path $destination "rtk-wsl1.exe"
$backup = "$target.previous.exe"
$cmdFallback = Join-Path $destination "rtk-wsl.cmd"
$tokenizerRoot = Join-Path $temporaryRoot "tokenizer"

function Assert-Condition([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

try {
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    Set-Content -LiteralPath $cmdFallback -Value "legacy fallback"

    & $install -Destination $destination
    Assert-Condition (Test-Path -LiteralPath $target) "fresh install did not create the WAD launcher"
    Assert-Condition (-not (Test-Path -LiteralPath $tokenizerRoot)) "fresh WAD install unexpectedly provisioned the optional tokenizer"
    & $install -Destination $destination -CommandName rtk-wsl
    Assert-Condition (Test-Path -LiteralPath $legacyTarget) "legacy compatibility alias install did not create the launcher"
    & $install -Destination $destination -CommandName rtk-wsl1
    Assert-Condition (Test-Path -LiteralPath $wsl1Target) "WSL1 alias install did not create the launcher"

    $reinstallRejected = $false
    try { & $install -Destination $destination } catch { $reinstallRejected = $true }
    Assert-Condition $reinstallRejected "install without -Force was not rejected"

    Set-Content -LiteralPath $target -Value "old launcher"
    & $install -Destination $destination -Force
    Assert-Condition (Test-Path -LiteralPath $backup) "upgrade did not retain a backup"
    Assert-Condition ((Get-Content -LiteralPath $backup -Raw) -eq "old launcher`r`n") "backup content changed"

    & $uninstall -Destination $destination -RestorePrevious
    Assert-Condition ((Get-Content -LiteralPath $target -Raw) -eq "old launcher`r`n") "rollback did not restore the previous launcher"

    & $install -Destination $destination -Force
    & $uninstall -Destination $destination
    Assert-Condition (-not (Test-Path -LiteralPath $target)) "uninstall did not remove the launcher"
    Assert-Condition (Test-Path -LiteralPath $legacyTarget) "WAD uninstall removed the legacy compatibility alias"

    & $uninstall -Destination $destination -CommandName rtk-wsl
    Assert-Condition (-not (Test-Path -LiteralPath $legacyTarget)) "legacy alias uninstall did not remove the launcher"
    Assert-Condition (Test-Path -LiteralPath $cmdFallback) "legacy uninstall removed the cmd fallback"

    & $uninstall -Destination $destination -CommandName rtk-wsl1
    Assert-Condition (-not (Test-Path -LiteralPath $wsl1Target)) "WSL1 alias uninstall did not remove the launcher"

    $tokenizerDestination = Join-Path $temporaryRoot "tokenizer-opt-in"
    & $install -Destination $tokenizerDestination -InstallTokenizer -TokenizerRoot $tokenizerRoot
    Assert-Condition (Test-Path -LiteralPath (Join-Path $tokenizerRoot "Scripts\python.exe")) "explicit tokenizer install did not provision the optional dependency"
    Assert-Condition (Test-Path -LiteralPath (Join-Path $tokenizerDestination "rtk-wad.exe")) "explicit tokenizer install did not create the WAD launcher"

    $tokenizerFailureDestination = Join-Path $temporaryRoot "tokenizer-failure"
    $missingPython = Join-Path $temporaryRoot "missing-python.exe"
    $tokenizerFailureRaised = $false
    try { & $install -Destination $tokenizerFailureDestination -InstallTokenizer -TokenizerPython $missingPython } catch { $tokenizerFailureRaised = $true }
    Assert-Condition $tokenizerFailureRaised "missing optional tokenizer runtime did not fail its explicit install"
    Assert-Condition (-not (Test-Path -LiteralPath (Join-Path $tokenizerFailureDestination "rtk-wad.exe"))) "tokenizer failure activated a WAD launcher"

    Set-Content -LiteralPath $target -Value "surviving launcher"
    $missingSource = Join-Path $temporaryRoot "missing.exe"
    $failedSafely = $false
    try { & $install -Destination $destination -Force -Source $missingSource } catch { $failedSafely = $true }
    Assert-Condition $failedSafely "missing source did not fail"
    Assert-Condition ((Get-Content -LiteralPath $target -Raw) -eq "surviving launcher`r`n") "failed install damaged the active launcher"

    Write-Output "Packaging contract passed"
    exit 0
} finally {
    if (Test-Path -LiteralPath $temporaryRoot) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
