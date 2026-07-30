[CmdletBinding()]
param(
    [string]$RepositoryRoot = (Split-Path -Parent $PSScriptRoot),
    [string]$Source = (Join-Path (Split-Path -Parent $PSScriptRoot) "target\release\xuva.exe")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$install = Join-Path $RepositoryRoot "scripts\install.ps1"
$uninstall = Join-Path $RepositoryRoot "scripts\uninstall.ps1"
$verifyPackage = Join-Path $RepositoryRoot "scripts\verify-package.ps1"
$packageRelease = Join-Path $RepositoryRoot "scripts\package-release.ps1"
$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) "xuva-packaging-$PID"
$destination = Join-Path $temporaryRoot "bin"
$previousDirectory = "$destination.previous"
$target = Join-Path $destination "xuva.exe"
$previousTarget = Join-Path $previousDirectory "xuva.exe"
$tokenizerRoot = Join-Path $temporaryRoot "tokenizer"

function Assert-Condition([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

function Get-BundleFingerprint([string]$Directory) {
    if (-not (Test-Path -LiteralPath $Directory)) { return "<missing>" }
    return (@(
        Get-ChildItem -LiteralPath $Directory -File |
            Sort-Object Name |
            ForEach-Object {
                "$($_.Name):$((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash)"
            }
    ) -join "`n")
}

try {
    $rootRejected = $false
    try {
        & $install -Destination ([System.IO.Path]::GetPathRoot($temporaryRoot)) -Status
    } catch {
        $rootRejected = $true
    }
    Assert-Condition $rootRejected "installer accepted a filesystem root as its bundle directory"

    & $install -Destination $destination -SkipProviderScan
    Assert-Condition (Test-Path -LiteralPath $target) "fresh install did not create the XUVA launcher"
    Assert-Condition (-not (Test-Path -LiteralPath $tokenizerRoot)) "fresh install unexpectedly provisioned the optional tokenizer"
    foreach ($companion in @("install.ps1", "uninstall.ps1", "verify-package.ps1", "xuva-wsl.sh")) {
        Assert-Condition (Test-Path -LiteralPath (Join-Path $destination $companion)) "fresh install omitted $companion"
    }
    $status = & $install -Destination $destination -Status | ConvertFrom-Json
    Assert-Condition ([bool]$status.Installed) "installer status did not report the active bundle"

    Add-Content -LiteralPath (Join-Path $destination "install.ps1") -Value "# previous-bundle-marker"
    Add-Content -LiteralPath (Join-Path $destination "uninstall.ps1") -Value "# previous-bundle-marker"
    Add-Content -LiteralPath (Join-Path $destination "xuva-wsl.sh") -Value "# previous-bundle-marker"
    & $install -Destination $destination -Force -SkipProviderScan
    Assert-Condition (Test-Path -LiteralPath $previousTarget) "upgrade did not retain the previous complete bundle"
    foreach ($companion in @("install.ps1", "uninstall.ps1", "xuva-wsl.sh")) {
        Assert-Condition ((Get-Content -LiteralPath (Join-Path $previousDirectory $companion) -Raw).Contains("previous-bundle-marker")) "backup lost $companion from the previous bundle"
    }

    $activeTimestamp = [datetime]::UtcNow.AddMinutes(-1)
    $previousTimestamp = [datetime]::UtcNow.AddYears(-1)
    (Get-Item -LiteralPath $target).LastWriteTimeUtc = $activeTimestamp
    (Get-Item -LiteralPath $previousTarget).LastWriteTimeUtc = $previousTimestamp
    & $target rollback | Out-Null
    $rollbackDeadline = [datetime]::UtcNow.AddSeconds(15)
    while (((Get-Item -LiteralPath $target).LastWriteTimeUtc -ne $previousTimestamp) -and
        ([datetime]::UtcNow -lt $rollbackDeadline)) {
        Start-Sleep -Milliseconds 100
    }
    Assert-Condition ((Get-Item -LiteralPath $target).LastWriteTimeUtc -eq $previousTimestamp) "binary lifecycle rollback did not swap the complete bundle"
    foreach ($companion in @("install.ps1", "uninstall.ps1", "xuva-wsl.sh")) {
        Assert-Condition ((Get-Content -LiteralPath (Join-Path $destination $companion) -Raw).Contains("previous-bundle-marker")) "rollback did not restore matching $companion"
    }

    $reinstallRejected = $false
    try { & $install -Destination $destination -SkipProviderScan } catch { $reinstallRejected = $true }
    Assert-Condition $reinstallRejected "install without -Force was not rejected"

    & $install -Destination $destination -Force -SkipProviderScan
    $activeBeforeFailure = Get-BundleFingerprint $destination
    $previousBeforeFailure = Get-BundleFingerprint $previousDirectory
    foreach ($failurePoint in @(
        "copy-binary",
        "copy-install",
        "copy-uninstall",
        "copy-shim",
        "path-update",
        "provider-scan"
    )) {
        $env:XUVA_TEST_MODE = "1"
        $env:XUVA_TEST_INSTALL_FAILURE = $failurePoint
        $raised = $false
        try {
            $arguments = @{
                Destination = $destination
                Force = $true
            }
            if ($failurePoint -eq "path-update") {
                $arguments.AddToPath = $true
                $arguments.SkipProviderScan = $true
            } elseif ($failurePoint -ne "provider-scan") {
                $arguments.SkipProviderScan = $true
            }
            & $install @arguments
        } catch {
            $raised = $true
        } finally {
            Remove-Item Env:XUVA_TEST_MODE -ErrorAction SilentlyContinue
            Remove-Item Env:XUVA_TEST_INSTALL_FAILURE -ErrorAction SilentlyContinue
        }
        Assert-Condition $raised "failure injection $failurePoint did not fail"
        Assert-Condition ((Get-BundleFingerprint $destination) -eq $activeBeforeFailure) "$failurePoint changed the active bundle"
        Assert-Condition ((Get-BundleFingerprint $previousDirectory) -eq $previousBeforeFailure) "$failurePoint changed the rollback bundle"
    }

    $activeBeforeRollbackFailure = Get-BundleFingerprint $destination
    $previousBeforeRollbackFailure = Get-BundleFingerprint $previousDirectory
    foreach ($failurePoint in @(
        "rollback-after-current-move",
        "rollback-after-previous-activate"
    )) {
        $env:XUVA_TEST_MODE = "1"
        $env:XUVA_TEST_INSTALL_FAILURE = $failurePoint
        $raised = $false
        try { & $install -Destination $destination -Rollback } catch { $raised = $true }
        finally {
            Remove-Item Env:XUVA_TEST_MODE -ErrorAction SilentlyContinue
            Remove-Item Env:XUVA_TEST_INSTALL_FAILURE -ErrorAction SilentlyContinue
        }
        Assert-Condition $raised "rollback failure injection $failurePoint did not fail"
        Assert-Condition ((Get-BundleFingerprint $destination) -eq $activeBeforeRollbackFailure) "$failurePoint changed the active bundle"
        Assert-Condition ((Get-BundleFingerprint $previousDirectory) -eq $previousBeforeRollbackFailure) "$failurePoint changed the rollback bundle"
    }
    & $install -Destination $destination -Rollback
    Assert-Condition (Test-Path -LiteralPath $target) "direct rollback removed the current bundle"
    Assert-Condition (Test-Path -LiteralPath $previousTarget) "direct rollback removed the previous bundle"
    $activeBeforeUninstallFailure = Get-BundleFingerprint $destination
    $previousBeforeUninstallFailure = Get-BundleFingerprint $previousDirectory
    foreach ($failurePoint in @("after-current-move", "after-previous-move")) {
        $env:XUVA_TEST_MODE = "1"
        $env:XUVA_TEST_UNINSTALL_FAILURE = $failurePoint
        $raised = $false
        try { & $uninstall -Destination $destination } catch { $raised = $true }
        finally {
            Remove-Item Env:XUVA_TEST_MODE -ErrorAction SilentlyContinue
            Remove-Item Env:XUVA_TEST_UNINSTALL_FAILURE -ErrorAction SilentlyContinue
        }
        Assert-Condition $raised "uninstall failure injection $failurePoint did not fail"
        Assert-Condition ((Get-BundleFingerprint $destination) -eq $activeBeforeUninstallFailure) "$failurePoint changed the active bundle"
        Assert-Condition ((Get-BundleFingerprint $previousDirectory) -eq $previousBeforeUninstallFailure) "$failurePoint changed the rollback bundle"
    }
    & $uninstall -Destination $destination
    Assert-Condition (-not (Test-Path -LiteralPath $destination)) "uninstall did not remove the current bundle"
    Assert-Condition (-not (Test-Path -LiteralPath $previousDirectory)) "uninstall did not remove the previous bundle"

    $tokenizerDestination = Join-Path $temporaryRoot "tokenizer-opt-in"
    & $install -Destination $tokenizerDestination -InstallTokenizer -TokenizerRoot $tokenizerRoot -SkipProviderScan
    Assert-Condition (Test-Path -LiteralPath (Join-Path $tokenizerRoot "Scripts\python.exe")) "explicit tokenizer install did not provision the optional dependency"
    Assert-Condition (Test-Path -LiteralPath (Join-Path $tokenizerDestination "xuva.exe")) "explicit tokenizer install did not create the XUVA launcher"

    $tokenizerFailureDestination = Join-Path $temporaryRoot "tokenizer-failure"
    $tokenizerFailureRoot = Join-Path $temporaryRoot "tokenizer-failure-root"
    $missingPython = Join-Path $temporaryRoot "missing-python.exe"
    $tokenizerFailureRaised = $false
    try {
        & $install -Destination $tokenizerFailureDestination -InstallTokenizer `
            -TokenizerRoot $tokenizerFailureRoot -TokenizerPython $missingPython -SkipProviderScan
    } catch {
        $tokenizerFailureRaised = $true
    }
    Assert-Condition $tokenizerFailureRaised "missing optional tokenizer runtime did not fail its explicit install"
    Assert-Condition (-not (Test-Path -LiteralPath $tokenizerFailureDestination)) "tokenizer failure activated a partial bundle"

    & $install -Destination $destination -SkipProviderScan
    $survivingFingerprint = Get-BundleFingerprint $destination
    foreach ($badSource in @(
        (Join-Path $temporaryRoot "missing.exe"),
        (Join-Path $temporaryRoot "invalid-xuva.exe")
    )) {
        if ($badSource.EndsWith("invalid-xuva.exe")) {
            Set-Content -LiteralPath $badSource -Value "not a Windows executable"
        }
        $failedSafely = $false
        try {
            & $install -Destination $destination -Force -Source $badSource -SkipProviderScan
        } catch {
            $failedSafely = $true
        }
        Assert-Condition $failedSafely "invalid or missing source did not fail"
        Assert-Condition ((Get-BundleFingerprint $destination) -eq $survivingFingerprint) "failed source install changed the active bundle"
    }

    $cargoVersion = (Select-String -LiteralPath (Join-Path $RepositoryRoot "Cargo.toml") `
        -Pattern '^version = "([^"]+)"$').Matches.Groups[1].Value
    $dist = Join-Path $temporaryRoot "dist"
    $archive = & $packageRelease -Version "v$cargoVersion" -Root $RepositoryRoot -OutputDirectory $dist
    Assert-Condition (Test-Path -LiteralPath $archive -PathType Leaf) "release packager did not create an archive"
    Assert-Condition (Test-Path -LiteralPath "$archive.sha256" -PathType Leaf) "release packager omitted its checksum"
    $archiveEntries = @(tar -tf $archive)
    $requiredEntries = @(
        "xuva.exe",
        "install.ps1",
        "install-tokenizer.ps1",
        "uninstall.ps1",
        "verify-package.ps1",
        "xuva-tokenizer.txt",
        "xuva-wsl.sh",
        "LICENSE",
        "SECURITY.md",
        "README.txt",
        "RELEASE-METADATA.json",
        "SHA256SUMS"
    )
    foreach ($required in $requiredEntries) {
        Assert-Condition ($archiveEntries -contains $required) "release archive omitted $required"
    }
    Assert-Condition ($archiveEntries.Count -eq $requiredEntries.Count) "release archive contains unreviewed files"
    $expandedArchive = Join-Path $temporaryRoot "expanded-release"
    Expand-Archive -LiteralPath $archive -DestinationPath $expandedArchive
    & $verifyPackage -PackageDirectory $expandedArchive -ExpectedVersion $cargoVersion

    Add-Content -LiteralPath (Join-Path $expandedArchive "uninstall.ps1") -Value "# tampered"
    $tamperRejected = $false
    try { & $verifyPackage -PackageDirectory $expandedArchive } catch { $tamperRejected = $true }
    Assert-Condition $tamperRejected "companion-script tampering was not rejected"

    $semanticArchive = Join-Path $temporaryRoot "semantic-release"
    Expand-Archive -LiteralPath $archive -DestinationPath $semanticArchive
    $metadataPath = Join-Path $semanticArchive "RELEASE-METADATA.json"
    $metadata = Get-Content -LiteralPath $metadataPath -Raw | ConvertFrom-Json
    $metadata.provenance = "untrusted-builder"
    $metadata | ConvertTo-Json | Set-Content -LiteralPath $metadataPath -Encoding utf8
    $metadataHash = (Get-FileHash -LiteralPath $metadataPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $checksumPath = Join-Path $semanticArchive "SHA256SUMS"
    $checksums = @(Get-Content -LiteralPath $checksumPath | ForEach-Object {
        if ($_ -match '  RELEASE-METADATA\.json$') {
            "$metadataHash  RELEASE-METADATA.json"
        } else {
            $_
        }
    })
    Set-Content -LiteralPath $checksumPath -Value $checksums -Encoding ascii
    $provenanceRejected = $false
    try { & $verifyPackage -PackageDirectory $semanticArchive } catch { $provenanceRejected = $true }
    Assert-Condition $provenanceRejected "invalid package provenance survived semantic verification"

    $nestedArchive = Join-Path $temporaryRoot "nested-release"
    Expand-Archive -LiteralPath $archive -DestinationPath $nestedArchive
    New-Item -ItemType Directory -Path (Join-Path $nestedArchive "unreviewed") | Out-Null
    $nestedRejected = $false
    try { & $verifyPackage -PackageDirectory $nestedArchive } catch { $nestedRejected = $true }
    Assert-Condition $nestedRejected "nested unreviewed package content was not rejected"

    Write-Output "Packaging contract passed"
    exit 0
} finally {
    Remove-Item Env:XUVA_TEST_MODE -ErrorAction SilentlyContinue
    Remove-Item Env:XUVA_TEST_INSTALL_FAILURE -ErrorAction SilentlyContinue
    Remove-Item Env:XUVA_TEST_UNINSTALL_FAILURE -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $temporaryRoot) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
