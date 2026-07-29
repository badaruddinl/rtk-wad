[CmdletBinding()]
param(
    [string]$RepositoryRoot = (Split-Path -Parent $PSScriptRoot),
    [string]$Source = (Join-Path (Split-Path -Parent $PSScriptRoot) "target\release\xuva.exe")
)

$ErrorActionPreference = "Stop"
$install = Join-Path $RepositoryRoot "scripts\install.ps1"
$uninstall = Join-Path $RepositoryRoot "scripts\uninstall.ps1"
$packageRelease = Join-Path $RepositoryRoot "scripts\package-release.ps1"
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
    Assert-Condition (-not (Test-Path -LiteralPath $tokenizerRoot)) "fresh XUVA install unexpectedly provisioned the optional tokenizer"
    Assert-Condition (Test-Path -LiteralPath (Join-Path $destination "uninstall.ps1")) "fresh install omitted its uninstaller"
    $status = & $install -Destination $destination -Status | ConvertFrom-Json
    Assert-Condition ([bool]$status.Installed) "installer status did not report the active launcher"
    & (Join-Path $destination "install.ps1") -Destination $destination -Force -SkipProviderScan
    Assert-Condition (Test-Path -LiteralPath $target) "in-place upgrade from the installed archive failed"
    $binaryStatus = & $target install --status | ConvertFrom-Json
    Assert-Condition ([bool]$binaryStatus.installer_available) "binary lifecycle status omitted its installed companion"
    $activeTimestamp = [datetime]::UtcNow.AddMinutes(-1)
    $previousTimestamp = [datetime]::UtcNow.AddYears(-1)
    (Get-Item -LiteralPath $target).LastWriteTimeUtc = $activeTimestamp
    (Get-Item -LiteralPath $backup).LastWriteTimeUtc = $previousTimestamp
    & $target rollback | Out-Null
    $rollbackDeadline = [datetime]::UtcNow.AddSeconds(15)
    while (((Get-Item -LiteralPath $target).LastWriteTimeUtc -ne $previousTimestamp) -and
        ([datetime]::UtcNow -lt $rollbackDeadline)) {
        Start-Sleep -Milliseconds 100
    }
    Assert-Condition ((Get-Item -LiteralPath $target).LastWriteTimeUtc -eq $previousTimestamp) "binary lifecycle rollback did not activate the retained launcher"

    $reinstallRejected = $false
    try { & $install -Destination $destination } catch { $reinstallRejected = $true }
    Assert-Condition $reinstallRejected "install without -Force was not rejected"

    Set-Content -LiteralPath $target -Value "old launcher"
    Set-Content -LiteralPath $backup -Value "stale backup"
    & $install -Destination $destination -Force
    Assert-Condition (Test-Path -LiteralPath $backup) "upgrade did not retain a backup"
    Assert-Condition ((Get-Content -LiteralPath $backup -Raw) -eq "old launcher`r`n") "backup content changed"

    & $install -Destination $destination -Rollback
    Assert-Condition ((Get-Content -LiteralPath $target -Raw) -eq "old launcher`r`n") "rollback did not restore the previous launcher"

    & $install -Destination $destination -Force
    & $uninstall -Destination $destination
    Assert-Condition (-not (Test-Path -LiteralPath $target)) "uninstall did not remove the launcher"
    Assert-Condition (-not (Test-Path -LiteralPath $backup)) "uninstall did not remove the rollback backup"

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

    $cargoVersion = (Select-String -LiteralPath (Join-Path $RepositoryRoot "Cargo.toml") `
        -Pattern '^version = "([^"]+)"$').Matches.Groups[1].Value
    $dist = Join-Path $temporaryRoot "dist"
    $archive = & $packageRelease -Version "v$cargoVersion" -Root $RepositoryRoot -OutputDirectory $dist
    Assert-Condition (Test-Path -LiteralPath $archive -PathType Leaf) "release packager did not create an archive"
    Assert-Condition (Test-Path -LiteralPath "$archive.sha256" -PathType Leaf) "release packager omitted its checksum"
    $archiveEntries = @(tar -tf $archive)
    $requiredEntries = @("xuva.exe", "install.ps1", "uninstall.ps1", "xuva-wsl.sh", "LICENSE", "SECURITY.md", "README.txt", "SHA256SUMS")
    foreach ($required in $requiredEntries) {
        Assert-Condition ($archiveEntries -contains $required) "release archive omitted $required"
    }
    Assert-Condition ($archiveEntries.Count -eq $requiredEntries.Count) "release archive contains unreviewed files"
    $expandedArchive = Join-Path $temporaryRoot "expanded-release"
    Expand-Archive -LiteralPath $archive -DestinationPath $expandedArchive
    foreach ($line in Get-Content -LiteralPath (Join-Path $expandedArchive "SHA256SUMS")) {
        $fields = $line -split '  ', 2
        Assert-Condition ($fields.Count -eq 2) "release payload checksum has an invalid record"
        $actual = (Get-FileHash -LiteralPath (Join-Path $expandedArchive $fields[1]) -Algorithm SHA256).Hash.ToLowerInvariant()
        Assert-Condition ($actual -eq $fields[0]) "release payload checksum mismatch for $($fields[1])"
    }

    Write-Output "Packaging contract passed"
    exit 0
} finally {
    if (Test-Path -LiteralPath $temporaryRoot) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
