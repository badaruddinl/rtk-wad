[CmdletBinding()]
param(
    [string]$CargoManifest = (Join-Path $PSScriptRoot "..\Cargo.toml")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$manifestPath = (Resolve-Path -LiteralPath $CargoManifest -ErrorAction Stop).Path
$matches = @(
    Select-String -LiteralPath $manifestPath -Pattern '^version = "([^"]+)"$'
)
if ($matches.Count -ne 1 -or $matches[0].Matches.Count -ne 1) {
    throw "Cargo manifest must contain exactly one package version."
}

$version = $matches[0].Matches[0].Groups[1].Value
if ($version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$') {
    throw "Cargo package version is not a supported release version: $version"
}

Write-Output $version
