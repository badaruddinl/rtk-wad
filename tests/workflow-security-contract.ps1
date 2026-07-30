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
