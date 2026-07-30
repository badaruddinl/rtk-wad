[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string]$PackageDirectory,
    [string]$ExpectedVersion,
    [string]$ExpectedCommit,
    [switch]$RequireGitHubProvenance
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = (Resolve-Path -LiteralPath $PackageDirectory).Path
$directories = @(Get-ChildItem -LiteralPath $root -Directory)
if ($directories.Count) {
    throw "Package must be a flat exact file set; nested directories are not permitted."
}
$allowed = @(
    "LICENSE",
    "README.txt",
    "RELEASE-METADATA.json",
    "SECURITY.md",
    "SHA256SUMS",
    "install.ps1",
    "install-tokenizer.ps1",
    "uninstall.ps1",
    "verify-package.ps1",
    "xuva-tokenizer.txt",
    "xuva-wsl.sh",
    "xuva.exe"
)
$actual = @(Get-ChildItem -LiteralPath $root -File | ForEach-Object Name | Sort-Object)
$unexpected = @($actual | Where-Object { $_ -notin $allowed })
$missing = @($allowed | Where-Object { $_ -notin $actual })
if ($unexpected.Count -or $missing.Count) {
    throw "Package file set mismatch. Missing=[$($missing -join ', ')] unexpected=[$($unexpected -join ', ')]."
}

$expectedPayload = @($allowed | Where-Object { $_ -ne "SHA256SUMS" } | Sort-Object)
$hashes = @{}
foreach ($line in Get-Content -LiteralPath (Join-Path $root "SHA256SUMS")) {
    if ($line -notmatch '^([0-9a-f]{64})  ([^/\\]+)$') {
        throw "Malformed SHA256SUMS entry: $line"
    }
    $name = $Matches[2]
    if ($hashes.ContainsKey($name)) {
        throw "Duplicate SHA256SUMS entry for $name."
    }
    $hashes[$name] = $Matches[1]
}
$hashedPayload = @($hashes.Keys | Sort-Object)
if (($hashedPayload -join "`n") -ne ($expectedPayload -join "`n")) {
    throw "SHA256SUMS does not cover the exact package payload."
}
foreach ($name in $expectedPayload) {
    $actualHash = (Get-FileHash -LiteralPath (Join-Path $root $name) -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualHash -ne $hashes[$name]) {
        throw "SHA256 mismatch for $name."
    }
}

$metadata = Get-Content -LiteralPath (Join-Path $root "RELEASE-METADATA.json") -Raw |
    ConvertFrom-Json
if ($metadata.schema_version -ne 1 -or $metadata.product -ne "xuva") {
    throw "Unsupported or invalid release metadata."
}
if ($metadata.version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$' -or
    $metadata.commit -notmatch '^[0-9a-f]{40}$' -or
    $metadata.target -ne "x86_64-pc-windows-msvc" -or
    $metadata.profile -ne "release" -or
    $metadata.provenance -notmatch '^(?:local-build|github-actions:[0-9]+)$') {
    throw "Release metadata has an invalid version, commit, target, profile, or provenance."
}
if ($ExpectedVersion -and $metadata.version -ne $ExpectedVersion.TrimStart("v")) {
    throw "Package version $($metadata.version) differs from expected version $ExpectedVersion."
}
if ($ExpectedCommit -and $metadata.commit -ne $ExpectedCommit.ToLowerInvariant()) {
    throw "Package commit $($metadata.commit) differs from expected commit $ExpectedCommit."
}
if ($RequireGitHubProvenance -and $metadata.provenance -notmatch '^github-actions:[0-9]+$') {
    throw "Official release package must carry GitHub Actions provenance."
}

$binary = Join-Path $root "xuva.exe"
$verbose = @(& $binary --version --verbose)
if ($LASTEXITCODE -ne 0) {
    throw "Packaged binary failed its verbose version check."
}
$reported = @{}
foreach ($line in $verbose) {
    if ($line -match '^([^=]+)=(.*)$') {
        $reported[$Matches[1]] = $Matches[2]
    }
}
if ($verbose[0] -ne "xuva $($metadata.version)" -or
    $reported["commit"] -ne $metadata.commit -or
    $reported["target"] -ne $metadata.target -or
    $reported["profile"] -ne $metadata.profile -or
    $reported["provenance"] -ne $metadata.provenance) {
    throw "Packaged binary identity differs from RELEASE-METADATA.json."
}

Write-Output "Verified XUVA package $($metadata.version) at commit $($metadata.commit)."
