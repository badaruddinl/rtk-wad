[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string]$DistributionDirectory,
    [Parameter(Mandatory)] [string]$ExpectedVersion,
    [Parameter(Mandatory)] [string]$ExpectedCommit
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$dist = (Resolve-Path -LiteralPath $DistributionDirectory).Path
$unexpectedDirectories = @(Get-ChildItem -LiteralPath $dist -Directory)
if ($unexpectedDirectories.Count) {
    throw "Release artifact set must be flat and must not contain unreviewed directories."
}
$version = $ExpectedVersion.TrimStart("v")
$archiveName = "xuva-v$version-windows-x86_64.zip"
$sidecarName = "$archiveName.sha256"
$sbomName = "xuva-v$version.cdx.json"
$toolchainName = "xuva-v$version.toolchain.json"
$expectedFiles = @($archiveName, $sidecarName, $sbomName, $toolchainName) | Sort-Object
$actualFiles = @(Get-ChildItem -LiteralPath $dist -File | ForEach-Object Name | Sort-Object)
if (($actualFiles -join "`n") -ne ($expectedFiles -join "`n")) {
    throw "Release artifact file set differs from the exact expected archive, digest, and SBOM."
}

$archive = Join-Path $dist $archiveName
$sidecar = (Get-Content -LiteralPath (Join-Path $dist $sidecarName) -Raw).Trim()
if ($sidecar -notmatch '^([0-9a-f]{64})  ([^/\\]+)$' -or $Matches[2] -ne $archiveName) {
    throw "Release archive checksum sidecar is malformed."
}
$actualHash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actualHash -ne $Matches[1]) {
    throw "Release archive checksum does not match its sidecar."
}

$sbom = Get-Content -LiteralPath (Join-Path $dist $sbomName) -Raw | ConvertFrom-Json
if (-not $sbom.bomFormat -or $sbom.bomFormat -ne "CycloneDX") {
    throw "Release SBOM is not valid CycloneDX JSON."
}
$toolchain = Get-Content -LiteralPath (Join-Path $dist $toolchainName) -Raw | ConvertFrom-Json
if ($toolchain.schema_version -ne 1 -or
    $toolchain.source_commit -ne $ExpectedCommit -or
    $toolchain.rustc -notmatch '^rustc 1\.97\.1 ' -or
    $toolchain.cargo -notmatch '^cargo 1\.97\.1 ' -or
    $toolchain.cargo_audit -ne "cargo-audit 0.22.2" -or
    $toolchain.cargo_cyclonedx -ne "cargo-cyclonedx-cyclonedx 0.5.9") {
    throw "Release toolchain provenance is incomplete or differs from the pinned toolchain."
}

$temporary = Join-Path ([System.IO.Path]::GetTempPath()) ("xuva-artifact-" + [guid]::NewGuid())
$expanded = Join-Path $temporary "expanded"
$installed = Join-Path $temporary "installed"
try {
    New-Item -ItemType Directory -Path $expanded -Force | Out-Null
    Expand-Archive -LiteralPath $archive -DestinationPath $expanded -Force
    & (Join-Path $PSScriptRoot "verify-package.ps1") -PackageDirectory $expanded `
        -ExpectedVersion $version -ExpectedCommit $ExpectedCommit -RequireGitHubProvenance
    if ($LASTEXITCODE -ne 0) {
        throw "Expanded package verification failed."
    }

    & (Join-Path $expanded "install.ps1") -Destination $installed -SkipProviderScan
    if ($LASTEXITCODE -ne 0) {
        throw "Release artifact installation failed."
    }
    $identity = @(& (Join-Path $installed "xuva.exe") --version --verbose)
    if ($LASTEXITCODE -ne 0 -or
        $identity[0] -ne "xuva $version" -or
        ($identity -join "`n") -notmatch "(?m)^commit=$([regex]::Escape($ExpectedCommit))$") {
        throw "Installed release artifact identity is incorrect."
    }
    & (Join-Path $installed "uninstall.ps1") -Destination $installed
    if ($LASTEXITCODE -ne 0 -or (Test-Path -LiteralPath $installed)) {
        throw "Release artifact uninstall verification failed."
    }
} finally {
    if (Test-Path -LiteralPath $temporary) {
        Remove-Item -LiteralPath $temporary -Recurse -Force
    }
}

Write-Output "Verified, installed, and removed the exact XUVA release artifact for $ExpectedCommit."
