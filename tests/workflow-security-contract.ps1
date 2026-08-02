[CmdletBinding()]
param(
    [string]$RepositoryRoot = (Join-Path $PSScriptRoot "..")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$verifier = Join-Path $RepositoryRoot "scripts\verify-workflow-shell-boundaries.ps1"
$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("xuva-workflow-security-" + [guid]::NewGuid())
try {
    & $verifier | Out-Null

    $versionReader = Join-Path $RepositoryRoot "scripts\read-cargo-version.ps1"
    $actualVersion = (& $versionReader).Trim()
    if ($actualVersion -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$') {
        throw "The repository Cargo version was not resolved canonically."
    }
    $releaseWorkflow = Get-Content -LiteralPath (
        Join-Path $RepositoryRoot ".github\workflows\release-provenance.yml"
    ) -Raw
    if ($releaseWorkflow -match 'Select-String\s+Cargo\.toml') {
        throw "Release version resolution regressed to ambiguous positional Select-String arguments."
    }
    if (
        [regex]::Matches($releaseWorkflow, 'scripts\\read-cargo-version\.ps1').Count -ne 2
    ) {
        throw "Every release version boundary must use the canonical Cargo version reader."
    }

    $testGateFiles = @(
        ".github\workflows\windows-ci.yml",
        ".github\workflows\release-provenance.yml",
        "scripts\verify-release.ps1"
    )
    foreach ($relativePath in $testGateFiles) {
        $testGate = Get-Content -LiteralPath (Join-Path $RepositoryRoot $relativePath) -Raw
        if ($testGate -notmatch 'cargo(?:Path)?\s+test\s+--locked\s+--lib\s+--bins') {
            throw "$relativePath must execute both library and binary unit tests."
        }
    }

    $wslContractWorkflow = Get-Content -LiteralPath (
        Join-Path $RepositoryRoot ".github\workflows\windows-wsl-self-hosted.yml"
    ) -Raw
    foreach ($profileVariable in @(
        'CARGO_INCREMENTAL',
        'CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG',
        'CARGO_PROFILE_DEV_DEBUG',
        'CARGO_PROFILE_TEST_BUILD_OVERRIDE_DEBUG',
        'CARGO_PROFILE_TEST_DEBUG'
    )) {
        if ([regex]::Matches($wslContractWorkflow, "(?m)^  ${profileVariable}: '0'\r?$").Count -ne 1) {
            throw "The WSL process contract must bound build artifacts with $profileVariable=0."
        }
    }
    if (
        [regex]::Matches($wslContractWorkflow, 'scripts\\ci-scratch\.ps1 -Mode prepare').Count -ne 2 -or
        [regex]::Matches($wslContractWorkflow, 'scripts\\ci-scratch\.ps1 -Mode cleanup').Count -ne 2
    ) {
        throw 'Both WSL process-contract jobs must prepare and reclaim isolated build scratch.'
    }
    if ($wslContractWorkflow -match '\.\\target\\debug\\xuva\.exe') {
        throw 'The WSL1 doctor gate must execute the launcher from CARGO_TARGET_DIR.'
    }
    if (
        $releaseWorkflow -match '(?m)^\s*path:\s*gated-dist\s*$' -or
        $releaseWorkflow -match 'DistributionDirectory\s+gated-dist' -or
        $releaseWorkflow -match 'Join-Path\s+gated-dist'
    ) {
        throw "Controlled release artifacts must not dirty the source checkout."
    }
    if (
        [regex]::Matches(
            $releaseWorkflow,
            '(?m)^\s*path:\s*\$\{\{\s*runner\.temp\s*\}\}/xuva-gated-dist-wsl[12]\s*$'
        ).Count -ne 2
    ) {
        throw "Both controlled WSL gates must stage artifacts below runner.temp."
    }

    New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
    $validManifest = Join-Path $temporaryRoot "valid-Cargo.toml"
    Set-Content -LiteralPath $validManifest -Value @'
[package]
name = "fixture"
version = "1.2.3-beta.4"
'@
    if ((& $versionReader -CargoManifest $validManifest).Trim() -ne "1.2.3-beta.4") {
        throw "The canonical Cargo version reader rejected a valid prerelease."
    }

    foreach ($invalidManifest in @(
        "[package]`nname = `"missing-version`"",
        "[package]`nversion = `"1.2.3`"`nversion = `"1.2.4`"",
        "[package]`nversion = `"not-semver`""
    )) {
        Set-Content -LiteralPath $validManifest -Value $invalidManifest
        $rejectedVersion = $false
        try {
            & $versionReader -CargoManifest $validManifest | Out-Null
        } catch {
            $rejectedVersion = $true
        }
        if (-not $rejectedVersion) {
            throw "An invalid or ambiguous Cargo version was accepted."
        }
    }

    $unsafe = @'
name: unsafe
jobs:
  release:
    runs-on: windows-latest
    steps:
      - run: Write-Output "${{ inputs.tag }}"
'@
    Set-Content -LiteralPath (Join-Path $temporaryRoot "unsafe.yml") -Value $unsafe
    $rejected = $false
    try {
        & $verifier -WorkflowDirectory $temporaryRoot | Out-Null
    } catch {
        $rejected = $_.Exception.Message.Contains("Pass it through step env instead")
    }
    if (-not $rejected) {
        throw "Direct workflow-input interpolation into shell source was not rejected."
    }

    $safe = @'
name: safe
jobs:
  release:
    runs-on: windows-latest
    steps:
      - env:
          RELEASE_TAG: ${{ inputs.tag }}
        run: Write-Output $env:RELEASE_TAG
'@
    Set-Content -LiteralPath (Join-Path $temporaryRoot "unsafe.yml") -Value $safe
    & $verifier -WorkflowDirectory $temporaryRoot | Out-Null
    Write-Output "Workflow security contract passed."
} finally {
    if ((Test-Path -LiteralPath $temporaryRoot) -and
        $temporaryRoot.StartsWith(
            [System.IO.Path]::GetTempPath(),
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
