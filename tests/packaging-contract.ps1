[CmdletBinding()]
param(
    [string]$RepositoryRoot = (Split-Path -Parent $PSScriptRoot),
    [string]$Source = (Join-Path (Split-Path -Parent $PSScriptRoot) "target\release\xuva.exe")
)

$ErrorActionPreference = "Stop"
$install = Join-Path $RepositoryRoot "scripts\install.ps1"
$uninstall = Join-Path $RepositoryRoot "scripts\uninstall.ps1"
$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) "xuva-packaging-$PID"
$destination = Join-Path $temporaryRoot "bin"
$target = Join-Path $destination "xuva.exe"
$backup = "$target.previous.exe"
$tokenizerRoot = Join-Path $temporaryRoot "tokenizer"

function Assert-Condition([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

try {
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    & $install -Destination $destination
    Assert-Condition (Test-Path -LiteralPath $target) "fresh install did not create the XUVA launcher"
    Assert-Condition (-not (Test-Path -LiteralPath $tokenizerRoot)) "fresh WAD install unexpectedly provisioned the optional tokenizer"

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

    $tokenizerDestination = Join-Path $temporaryRoot "tokenizer-opt-in"
    & $install -Destination $tokenizerDestination -InstallTokenizer -TokenizerRoot $tokenizerRoot
    Assert-Condition (Test-Path -LiteralPath (Join-Path $tokenizerRoot "Scripts\python.exe")) "explicit tokenizer install did not provision the optional dependency"
    Assert-Condition (Test-Path -LiteralPath (Join-Path $tokenizerDestination "xuva.exe")) "explicit tokenizer install did not create the XUVA launcher"

    $tokenizerFailureDestination = Join-Path $temporaryRoot "tokenizer-failure"
    $tokenizerFailureRoot = Join-Path $temporaryRoot "tokenizer-failure-root"
    $missingPython = Join-Path $temporaryRoot "missing-python.exe"
    $tokenizerFailureRaised = $false
    try { & $install -Destination $tokenizerFailureDestination -InstallTokenizer -TokenizerRoot $tokenizerFailureRoot -TokenizerPython $missingPython } catch { $tokenizerFailureRaised = $true }
    Assert-Condition $tokenizerFailureRaised "missing optional tokenizer runtime did not fail its explicit install"
    Assert-Condition (-not (Test-Path -LiteralPath (Join-Path $tokenizerFailureDestination "xuva.exe"))) "tokenizer failure activated an XUVA launcher"

    Set-Content -LiteralPath $target -Value "surviving launcher"
    $missingSource = Join-Path $temporaryRoot "missing.exe"
    $failedSafely = $false
    try { & $install -Destination $destination -Force -Source $missingSource } catch { $failedSafely = $true }
    Assert-Condition $failedSafely "missing source did not fail"
    Assert-Condition ((Get-Content -LiteralPath $target -Raw) -eq "surviving launcher`r`n") "failed install damaged the active launcher"

    $invalidSource = Join-Path $temporaryRoot "invalid-xuva.exe"
    Set-Content -LiteralPath $invalidSource -Value "not a Windows executable"
    $backupExistedBeforeInvalid = Test-Path -LiteralPath $backup
    $backupBeforeInvalid = if ($backupExistedBeforeInvalid) { Get-Content -LiteralPath $backup -Raw } else { $null }
    $invalidRejected = $false
    try { & $install -Destination $destination -Force -Source $invalidSource } catch { $invalidRejected = $true }
    Assert-Condition $invalidRejected "invalid candidate launcher was not rejected"
    Assert-Condition ((Get-Content -LiteralPath $target -Raw) -eq "surviving launcher`r`n") "candidate smoke-check failure damaged the active launcher"
    Assert-Condition ((Test-Path -LiteralPath $backup) -eq $backupExistedBeforeInvalid) "candidate smoke-check failure changed rollback-backup presence"
    if ($backupExistedBeforeInvalid) {
        Assert-Condition ((Get-Content -LiteralPath $backup -Raw) -eq $backupBeforeInvalid) "candidate smoke-check failure changed rollback-backup content"
    }

    Write-Output "Packaging contract passed"
    exit 0
} finally {
    if (Test-Path -LiteralPath $temporaryRoot) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
